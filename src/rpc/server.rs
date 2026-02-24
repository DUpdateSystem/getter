use super::data::*;
use crate::cache::init_cache_manager_with_expire;
use crate::core::config::world::{init_world_list, world_list};
use crate::database::get_db;
use crate::database::models::extra_hub::GLOBAL_HUB_ID;
use crate::downloader::{DownloadConfig, DownloadTaskManager};
use crate::manager::android_api;
use crate::manager::app_manager::AppManager;
use crate::manager::cloud_config_getter::CloudConfigGetter;
use crate::manager::hub_manager::HubManager;
use crate::manager::notification;
use crate::manager::url_replace::apply_url_replace;
use crate::websdk::cloud_rules::cloud_rules_manager::CloudRules;
use crate::websdk::repo::api;
use jsonrpsee::server::{RpcModule, Server, ServerConfig, ServerHandle};
use jsonrpsee::types::{ErrorCode, ErrorObjectOwned};
use once_cell::sync::OnceCell;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Global manager state initialised on first `init` RPC call.
static APP_MANAGER: OnceCell<Arc<RwLock<AppManager>>> = OnceCell::new();
static HUB_MANAGER: OnceCell<Arc<RwLock<HubManager>>> = OnceCell::new();
static CLOUD_CONFIG_GETTER: OnceCell<Arc<RwLock<CloudConfigGetter>>> = OnceCell::new();

fn get_app_manager() -> Option<Arc<RwLock<AppManager>>> {
    APP_MANAGER.get().cloned()
}

fn get_hub_manager() -> Option<Arc<RwLock<HubManager>>> {
    HUB_MANAGER.get().cloned()
}

fn get_cloud_config_getter() -> Option<Arc<RwLock<CloudConfigGetter>>> {
    CLOUD_CONFIG_GETTER.get().cloned()
}

fn manager_not_init_err() -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        ErrorCode::InternalError.code(),
        "Manager not initialized. Call init first.",
        None::<String>,
    )
}

fn map_manager_err(e: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        ErrorCode::InternalError.code(),
        e.to_string(),
        None::<String>,
    )
}

// Default 2GB size limit for WebSocket messages
// Can be overridden at runtime by setting GETTER_WS_MAX_MESSAGE_SIZE environment variable
// Example: GETTER_WS_MAX_MESSAGE_SIZE=1073741824 ./getter (for 1GB)
const DEFAULT_MAX_SIZE: u32 = 2 * 1024 * 1024 * 1024; // 2GB

fn get_max_message_size() -> u32 {
    // Allow runtime configuration via environment variable
    match std::env::var("GETTER_WS_MAX_MESSAGE_SIZE") {
        Ok(size_str) => size_str.parse().unwrap_or(DEFAULT_MAX_SIZE),
        Err(_) => DEFAULT_MAX_SIZE,
    }
}

pub async fn run_server(
    addr: &str,
    is_running: Arc<AtomicBool>,
) -> Result<(String, ServerHandle), Box<dyn std::error::Error>> {
    let addr = if addr.is_empty() { "127.0.0.1:0" } else { addr };
    let max_size = get_max_message_size();
    let config = ServerConfig::builder()
        .max_request_body_size(max_size)
        .max_response_body_size(max_size)
        .build();
    let server = Server::builder()
        .set_config(config)
        .build(addr.parse::<SocketAddr>()?)
        .await?;
    let mut module = RpcModule::new(());
    // Register the shutdown method
    let run_flag = is_running.clone();
    module.register_async_method("shutdown", move |_, _, _| {
        let flag = run_flag.clone();
        async move {
            flag.store(false, Ordering::SeqCst);
        }
    })?;
    module.register_method("ping", |_, _, _| "pong")?;
    module.register_async_method("init", |params, _, _| async move {
        let request = params.parse::<RpcInitRequest>()?;
        let data_dir = Path::new(request.data_path);
        let cache_dir = Path::new(request.cache_path);
        // Initialize world list, cache, and database.
        let world_list_path = data_dir.join(world_list::WORLD_CONFIG_LIST_NAME);
        init_world_list(&world_list_path).await.map_err(|e| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "Internal error",
                Some(e.to_string()),
            )
        })?;
        let local_cache_path = cache_dir.join("local_cache");
        init_cache_manager_with_expire(local_cache_path.as_path(), request.global_expire_time)
            .await;
        crate::database::init_db(data_dir).map_err(|e| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "Internal error",
                Some(e.to_string()),
            )
        })?;

        // Initialize managers (idempotent: only on first call)
        if APP_MANAGER.get().is_none() {
            let hub_mgr = HubManager::load().map_err(map_manager_err)?;
            let app_mgr = AppManager::load().map_err(map_manager_err)?;
            let _ = HUB_MANAGER.set(Arc::new(RwLock::new(hub_mgr)));
            let _ = APP_MANAGER.set(Arc::new(RwLock::new(app_mgr)));
        }

        Ok::<bool, ErrorObjectOwned>(true)
    })?;
    module.register_async_method(
        "check_app_available",
        |params, _context, _extensions| async move {
            let request = params.parse::<RpcAppRequest>()?;
            let result =
                api::check_app_available(request.hub_uuid, &request.app_data, &request.hub_data)
                    .await
                    .unwrap_or(false);
            Ok::<bool, ErrorObjectOwned>(result)
        },
    )?;
    module.register_async_method(
        "get_latest_release",
        |params, _context, _extensions| async move {
            let request = params.parse::<RpcAppRequest>()?;
            if let Some(result) =
                api::get_latest_release(request.hub_uuid, &request.app_data, &request.hub_data)
                    .await
            {
                Ok(result)
            } else {
                Err(ErrorObjectOwned::owned(
                    -32001,
                    "No release found",
                    None::<String>,
                ))
            }
        },
    )?;
    module.register_async_method("get_releases", |params, _context, _extensions| async move {
        let request = params.parse::<RpcAppRequest>()?;
        if let Some(result) =
            api::get_releases(request.hub_uuid, &request.app_data, &request.hub_data).await
        {
            Ok(result)
        } else {
            Err(ErrorObjectOwned::owned(
                -32001,
                "No releases found",
                None::<String>,
            ))
        }
    })?;

    // register_provider: Dynamically register an external provider (e.g., Kotlin hub via HTTP JSON-RPC)
    module.register_async_method(
        "register_provider",
        |params, _context, _extensions| async move {
            let request = params.parse::<RpcRegisterProviderRequest>()?;
            api::add_outside_provider(request.hub_uuid, request.url);
            Ok::<bool, ErrorObjectOwned>(true)
        },
    )?;

    // get_download: Get download info for an app's asset.
    // After retrieving download URLs from the provider, applies URL replacement
    // rules from ExtraHub configs (GLOBAL first, then hub-specific), mirroring
    // Kotlin's URLReplace.replaceURL() in the download pipeline.
    module.register_async_method("get_download", |params, _context, _extensions| async move {
        let request = params.parse::<RpcDownloadInfoRequest>()?;
        let mut items = api::get_download(
            request.hub_uuid,
            &request.app_data,
            &request.hub_data,
            &request.asset_index,
        )
        .await
        .ok_or_else(|| ErrorObjectOwned::owned(-32001, "No download info found", None::<String>))?;

        // Load URL-replace rules from ExtraHub configs.
        // Priority: hub-specific rule overrides GLOBAL rule.
        let db = get_db();
        let global_extra = db.find_extra_hub(GLOBAL_HUB_ID).unwrap_or(None);
        let hub_extra = db.find_extra_hub(request.hub_uuid).unwrap_or(None);

        // Apply rules to every download URL in the result.
        for item in &mut items {
            // Apply GLOBAL rule first (lower priority)
            if let Some(ref g) = global_extra {
                item.url = apply_url_replace(
                    &item.url,
                    g.url_replace_search.as_deref(),
                    g.url_replace_string.as_deref(),
                );
            }
            // Apply hub-specific rule second (higher priority, may override)
            if let Some(ref h) = hub_extra {
                item.url = apply_url_replace(
                    &item.url,
                    h.url_replace_search.as_deref(),
                    h.url_replace_string.as_deref(),
                );
            }
        }

        Ok::<Vec<DownloadItemData>, ErrorObjectOwned>(items)
    })?;

    module.register_async_method(
        "get_cloud_config",
        |params, _context, _extensions| async move {
            if let Ok(request) = params.parse::<RpcCloudConfigRequest>() {
                let mut cloud_rules = CloudRules::new(request.api_url);
                if let Err(e) = cloud_rules.renew().await {
                    return Err(ErrorObjectOwned::owned(
                        ErrorCode::InternalError.code(),
                        "Download cloud config failed",
                        Some(e.to_string()),
                    ));
                }
                Ok(cloud_rules.get_config_list().to_owned())
            } else {
                Err(ErrorObjectOwned::owned(
                    ErrorCode::ParseError.code(),
                    "Parse params error",
                    Some(params.as_str().unwrap_or("None").to_string()),
                ))
            }
        },
    )?;

    // ========================================================================
    // Downloader RPC Methods
    // ========================================================================

    // Create download task manager with HubDispatchDownloader
    let download_config = DownloadConfig::from_env();
    let http_downloader = crate::downloader::create_downloader(&download_config);
    let dispatcher = crate::downloader::HubDispatchDownloader::new(http_downloader);

    // Clone dispatcher for task manager (HubDispatchDownloader is cheap to clone via Arc internally)
    let task_manager = Arc::new(DownloadTaskManager::new(Box::new(dispatcher.clone())));

    // download_submit: Submit a single download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_submit", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcDownloadRequest>()?;
            match manager.submit_task_with_options(
                request.url,
                request.dest_path,
                request.headers,
                request.cookies,
                request.hub_uuid,
            ) {
                Ok(task_id) => Ok(RpcTaskIdResponse { task_id }),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to submit download task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_submit_batch: Submit multiple download tasks
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_submit_batch",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcDownloadBatchRequest>()?;
                let tasks: Vec<(String, String)> = request
                    .tasks
                    .into_iter()
                    .map(|t| (t.url, t.dest_path))
                    .collect();

                match manager.submit_batch(tasks) {
                    Ok(task_ids) => Ok(RpcTaskIdsResponse { task_ids }),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InternalError.code(),
                        "Failed to submit batch download tasks",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_get_status: Get status of a download task
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_status",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcTaskStatusRequest>()?;
                match manager.get_task(request.task_id) {
                    Ok(task_info) => Ok(task_info),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InvalidParams.code(),
                        "Task not found",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_wait_for_change: Long-polling for task state change
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_wait_for_change",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcWaitForChangeRequest>()?;
                let timeout = Duration::from_secs(request.timeout_seconds);

                match manager.wait_for_change(request.task_id, timeout).await {
                    Ok(task_info) => Ok(task_info),
                    Err(e) => Err(ErrorObjectOwned::owned(
                        ErrorCode::InvalidParams.code(),
                        "Failed to wait for task change",
                        Some(e.message),
                    )),
                }
            }
        },
    )?;

    // download_cancel: Cancel a download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_cancel", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcCancelTaskRequest>()?;
            match manager.cancel_task(request.task_id) {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to cancel task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_pause: Pause a download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_pause", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcPauseTaskRequest>()?;
            match manager.pause_task(request.task_id).await {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to pause task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_resume: Resume a paused download task
    let manager_clone = task_manager.clone();
    module.register_async_method("download_resume", move |params, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            let request = params.parse::<RpcResumeTaskRequest>()?;
            match manager.resume_task(request.task_id).await {
                Ok(_) => Ok(true),
                Err(e) => Err(ErrorObjectOwned::owned(
                    ErrorCode::InternalError.code(),
                    "Failed to resume task",
                    Some(e.message),
                )),
            }
        }
    })?;

    // download_get_capabilities: Get downloader capabilities
    let manager_clone = task_manager.clone();
    module.register_method(
        "download_get_capabilities",
        move |_, _context, _extensions| {
            let caps = manager_clone.get_capabilities();
            Ok::<_, ErrorObjectOwned>(caps.clone())
        },
    )?;

    // download_get_all_tasks: Get all tasks
    let manager_clone = task_manager.clone();
    module.register_async_method("download_get_all_tasks", move |_, _context, _extensions| {
        let manager = manager_clone.clone();
        async move {
            Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                tasks: manager.get_all_tasks(),
            })
        }
    })?;

    // download_get_active_tasks: Get active tasks
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_active_tasks",
        move |_, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                    tasks: manager.get_active_tasks(),
                })
            }
        },
    )?;

    // download_get_tasks_by_state: Get tasks by state
    let manager_clone = task_manager.clone();
    module.register_async_method(
        "download_get_tasks_by_state",
        move |params, _context, _extensions| {
            let manager = manager_clone.clone();
            async move {
                let request = params.parse::<RpcTasksByStateRequest>()?;
                Ok::<RpcTasksResponse, ErrorObjectOwned>(RpcTasksResponse {
                    tasks: manager.get_tasks_by_state(request.state),
                })
            }
        },
    )?;

    // register_downloader: Register an external downloader for a hub_uuid
    let dispatcher_clone = dispatcher.clone();
    module.register_async_method(
        "register_downloader",
        move |params, _context, _extensions| {
            let dispatcher = dispatcher_clone.clone();
            async move {
                let request = params.parse::<RpcRegisterDownloaderRequest>()?;
                let external_downloader = Box::new(crate::downloader::ExternalRpcDownloader::new(
                    request.rpc_url.to_string(),
                ));
                dispatcher.register(request.hub_uuid, external_downloader);
                Ok::<bool, ErrorObjectOwned>(true)
            }
        },
    )?;

    // unregister_downloader: Unregister an external downloader for a hub_uuid
    let dispatcher_clone = dispatcher.clone();
    module.register_async_method(
        "unregister_downloader",
        move |params, _context, _extensions| {
            let dispatcher = dispatcher_clone.clone();
            async move {
                let request = params.parse::<RpcUnregisterDownloaderRequest>()?;
                dispatcher.unregister(request.hub_uuid);
                Ok::<bool, ErrorObjectOwned>(true)
            }
        },
    )?;

    // ========================================================================
    // App Manager RPC Methods
    // ========================================================================

    // manager_get_apps: Get all saved apps
    module.register_async_method("manager_get_apps", |_, _, _| async move {
        let mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let apps = mgr.read().await.get_saved_apps().await;
        Ok::<Vec<crate::database::models::app::AppRecord>, ErrorObjectOwned>(apps)
    })?;

    // manager_save_app: Insert or update an app record
    module.register_async_method("manager_save_app", |params, _, _| async move {
        let request = params.parse::<RpcSaveAppRequest>()?;
        let mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let saved = mgr
            .write()
            .await
            .save_app(request.record)
            .await
            .map_err(map_manager_err)?;
        Ok::<crate::database::models::app::AppRecord, ErrorObjectOwned>(saved)
    })?;

    // manager_delete_app: Delete an app by record id
    module.register_async_method("manager_delete_app", |params, _, _| async move {
        let request = params.parse::<RpcDeleteAppRequest>()?;
        let mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let deleted = mgr
            .write()
            .await
            .remove_app(&request.record_id)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(deleted)
    })?;

    // manager_get_app_status: Get AppStatus for a specific app
    module.register_async_method("manager_get_app_status", |params, _, _| async move {
        let request = params.parse::<RpcGetAppStatusRequest>()?;
        let mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let status = mgr.write().await.get_app_status(&request.record_id).await;
        Ok::<crate::manager::app_status::AppStatus, ErrorObjectOwned>(status)
    })?;

    // manager_set_virtual_apps: Set installed (virtual) apps from Android
    module.register_async_method("manager_set_virtual_apps", |params, _, _| async move {
        let request = params.parse::<RpcSetVirtualAppsRequest>()?;
        let mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        mgr.read().await.set_virtual_apps(request.apps).await;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_renew_all: Trigger a full update check for all apps
    module.register_async_method("manager_renew_all", |_, _, _| async move {
        let app_mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let hub_mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let hubs = hub_mgr.read().await.get_hub_list().await;
        app_mgr.read().await.renew_all(&hubs, None).await;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_check_invalid_applications: Return record IDs of apps whose configured
    // hub UUIDs are all unknown (no valid hub found). Mirrors Kotlin's
    // AppManager.check_invalid_applications logic.
    module.register_async_method("manager_check_invalid_applications", |_, _, _| async move {
        let app_mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let hub_mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let hubs = hub_mgr.read().await.get_hub_list().await;
        let known_uuids: Vec<String> = hubs.into_iter().map(|h| h.uuid).collect();
        let mgr = app_mgr.read().await;
        let invalid_ids = mgr.check_invalid_applications(&known_uuids).await;
        // Notify Kotlin UI about each invalid app so it can update status.
        for record_id in &invalid_ids {
            if let Some(app) = mgr.get_app(record_id).await {
                notification::notify_if_registered(notification::ManagerEvent::AppStatusChanged {
                    record_id: record_id.clone(),
                    app_id: app.app_id.clone(),
                    old_status: crate::manager::app_status::AppStatus::AppLatest,
                    new_status: crate::manager::app_status::AppStatus::AppInactive,
                })
                .await;
            }
        }
        Ok::<Vec<String>, ErrorObjectOwned>(invalid_ids)
    })?;

    // ========================================================================
    // Hub Manager RPC Methods
    // ========================================================================

    // manager_get_hubs: Get all hubs
    module.register_async_method("manager_get_hubs", |_, _, _| async move {
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let hubs = mgr.read().await.get_hub_list().await;
        Ok::<Vec<crate::database::models::hub::HubRecord>, ErrorObjectOwned>(hubs)
    })?;

    // manager_save_hub: Insert or update a hub
    module.register_async_method("manager_save_hub", |params, _, _| async move {
        let request = params.parse::<RpcSaveHubRequest>()?;
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        mgr.write()
            .await
            .upsert_hub(request.record)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_update_hub_auth: Replace the auth map for a hub and persist.
    module.register_async_method("manager_update_hub_auth", |params, _, _| async move {
        let request = params.parse::<RpcUpdateHubAuthRequest>()?;
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let updated = mgr
            .read()
            .await
            .update_auth(&request.hub_uuid, request.auth)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(updated)
    })?;

    // manager_delete_hub: Delete a hub by UUID
    module.register_async_method("manager_delete_hub", |params, _, _| async move {
        let request = params.parse::<RpcDeleteHubRequest>()?;
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let deleted = mgr
            .write()
            .await
            .remove_hub(&request.hub_uuid)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(deleted)
    })?;

    // manager_hub_ignore_app: Add or remove an app from a hub's ignore list
    module.register_async_method("manager_hub_ignore_app", |params, _, _| async move {
        let request = params.parse::<RpcHubIgnoreAppRequest>()?;
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let mut guard = mgr.write().await;
        let mut hub = guard.get_hub(&request.hub_uuid).await.ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InvalidParams.code(),
                "Hub not found",
                None::<String>,
            )
        })?;
        if request.ignore {
            if !hub.user_ignore_app_id_list.contains(&request.app_id) {
                hub.user_ignore_app_id_list.push(request.app_id);
            }
        } else {
            hub.user_ignore_app_id_list
                .retain(|id| id != &request.app_id);
        }
        guard.upsert_hub(hub).await.map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_set_applications_mode: Enable/disable auto app discovery for a hub
    module.register_async_method("manager_set_applications_mode", |params, _, _| async move {
        let request = params.parse::<RpcSetApplicationsModeRequest>()?;
        let mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        let mut guard = mgr.write().await;
        let mut hub = guard.get_hub(&request.hub_uuid).await.ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InvalidParams.code(),
                "Hub not found",
                None::<String>,
            )
        })?;
        hub.applications_mode = if request.enable { 1 } else { 0 };
        guard.upsert_hub(hub).await.map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // ========================================================================
    // ExtraHub RPC Methods
    // ========================================================================

    // manager_get_extra_hubs: Get all extra hub configs
    module.register_async_method("manager_get_extra_hubs", |_, _, _| async move {
        let extra_hubs = crate::database::get_db()
            .load_extra_hubs()
            .map_err(map_manager_err)?;
        Ok::<Vec<crate::database::models::extra_hub::ExtraHubRecord>, ErrorObjectOwned>(extra_hubs)
    })?;

    // manager_save_extra_hub: Insert or update an extra hub config
    module.register_async_method("manager_save_extra_hub", |params, _, _| async move {
        let request = params.parse::<RpcSaveExtraHubRequest>()?;
        crate::database::get_db()
            .upsert_extra_hub(&request.record)
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_delete_extra_hub: Delete an extra hub by id
    module.register_async_method("manager_delete_extra_hub", |params, _, _| async move {
        let request = params.parse::<RpcGetExtraHubRequest>()?;
        let deleted = crate::database::get_db()
            .delete_extra_hub(&request.id)
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(deleted)
    })?;

    // ========================================================================
    // Android API / Notification Registration RPC Methods
    // ========================================================================

    // register_android_api: Register Kotlin's Android API callback URL
    module.register_async_method("register_android_api", |params, _, _| async move {
        let request = params.parse::<RpcRegisterAndroidApiRequest>()?;
        android_api::set_android_api(request.url);
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // register_notification: Register Kotlin's notification callback URL
    module.register_async_method("register_notification", |params, _, _| async move {
        let request = params.parse::<RpcRegisterNotificationRequest>()?;
        notification::set_notification(request.url);
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // ========================================================================
    // ExtraApp RPC Methods
    // ========================================================================

    // manager_get_extra_app_by_app_id: Get ExtraApp record by app_id map
    module.register_async_method(
        "manager_get_extra_app_by_app_id",
        |params, _, _| async move {
            let request = params.parse::<RpcGetExtraAppRequest>()?;
            let record = crate::database::get_db()
                .get_extra_app_by_app_id(&request.app_id)
                .map_err(map_manager_err)?;
            Ok::<Option<crate::database::models::extra_app::ExtraAppRecord>, ErrorObjectOwned>(
                record,
            )
        },
    )?;

    // manager_save_extra_app: Insert or update an ExtraApp record
    module.register_async_method("manager_save_extra_app", |params, _, _| async move {
        let request = params.parse::<RpcSaveExtraAppRequest>()?;
        crate::database::get_db()
            .upsert_extra_app(&request.record)
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // manager_delete_extra_app: Delete an ExtraApp by database id
    module.register_async_method("manager_delete_extra_app", |params, _, _| async move {
        let request = params.parse::<RpcDeleteExtraAppRequest>()?;
        let deleted = crate::database::get_db()
            .delete_extra_app(&request.id)
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(deleted)
    })?;

    // ========================================================================
    // Cloud Config Manager RPC Methods
    // ========================================================================

    // cloud_config_init: Initialise or re-initialise the CloudConfigGetter with an API URL
    module.register_async_method("cloud_config_init", |params, _, _| async move {
        let request = params.parse::<RpcCloudConfigInitRequest>()?;
        let getter = CloudConfigGetter::new(request.api_url);
        let _ = CLOUD_CONFIG_GETTER.set(Arc::new(RwLock::new(getter)));
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // cloud_config_renew: Download and cache the latest cloud config
    module.register_async_method("cloud_config_renew", |_, _, _| async move {
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised. Call cloud_config_init first.",
                None::<String>,
            )
        })?;
        getter.read().await.renew().await.map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // cloud_config_get_app_list: Return all available app configs from cache
    module.register_async_method("cloud_config_get_app_list", |_, _, _| async move {
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised.",
                None::<String>,
            )
        })?;
        let list = getter.read().await.app_config_list().await;
        Ok::<Vec<crate::websdk::cloud_rules::data::app_item::AppItem>, ErrorObjectOwned>(list)
    })?;

    // cloud_config_get_hub_list: Return all available hub configs from cache
    module.register_async_method("cloud_config_get_hub_list", |_, _, _| async move {
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised.",
                None::<String>,
            )
        })?;
        let list = getter.read().await.hub_config_list().await;
        Ok::<Vec<crate::websdk::cloud_rules::data::hub_item::HubItem>, ErrorObjectOwned>(list)
    })?;

    // cloud_config_apply_app: Apply a cloud app config by UUID
    module.register_async_method("cloud_config_apply_app", |params, _, _| async move {
        let request = params.parse::<RpcCloudConfigApplyRequest>()?;
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised.",
                None::<String>,
            )
        })?;
        let app_mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let hub_mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        getter
            .read()
            .await
            .apply_app_config(
                &request.uuid,
                &mut *app_mgr.write().await,
                &mut *hub_mgr.write().await,
            )
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // cloud_config_apply_hub: Apply a cloud hub config by UUID
    module.register_async_method("cloud_config_apply_hub", |params, _, _| async move {
        let request = params.parse::<RpcCloudConfigApplyRequest>()?;
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised.",
                None::<String>,
            )
        })?;
        let hub_mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        getter
            .read()
            .await
            .apply_hub_config(&request.uuid, &mut *hub_mgr.write().await)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    // cloud_config_renew_all: Bulk-update all installed apps/hubs from cloud
    module.register_async_method("cloud_config_renew_all", |_, _, _| async move {
        let getter = get_cloud_config_getter().ok_or_else(|| {
            ErrorObjectOwned::owned(
                ErrorCode::InternalError.code(),
                "CloudConfigGetter not initialised.",
                None::<String>,
            )
        })?;
        let app_mgr = get_app_manager().ok_or_else(manager_not_init_err)?;
        let hub_mgr = get_hub_manager().ok_or_else(manager_not_init_err)?;
        getter
            .read()
            .await
            .renew_all_from_cloud(&mut *app_mgr.write().await, &mut *hub_mgr.write().await)
            .await
            .map_err(map_manager_err)?;
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    let addr = server.local_addr()?;
    let handle = server.start(module);
    tokio::spawn(handle.clone().stopped());
    Ok((format!("http://{}", addr), handle))
}

#[allow(dead_code)]
pub async fn run_server_hanging<T>(
    addr: &str,
    callback: impl Fn(&str) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let is_running = Arc::new(AtomicBool::new(true));
    let (url, handle) = match run_server(addr, is_running.clone()).await {
        Ok((url, handle)) => (url, handle),
        Err(e) => {
            eprintln!("Failed to start server: {}", e);
            return Err(e);
        }
    };
    let result = callback(&url)?;
    while is_running.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    handle.stop()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::rpc::client::Client;
    use crate::websdk::repo::provider::github;
    use crate::websdk::{
        cloud_rules::data::config_list::ConfigList, repo::data::release::ReleaseData,
    };

    use super::*;
    use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder, rpc_params};
    use mockito::Server;
    use std::collections::BTreeMap;
    use std::fs;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_server_start() {
        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() > 0);
        handle.stop().unwrap();
        let port = 33333;
        let addr = format!("127.0.0.1:{}", port);
        let (url, handle) = run_server(&addr, Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        assert!(url.starts_with("http://"));
        assert!(url.split(":").last().unwrap().parse::<u16>().unwrap() == port);
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_ping() {
        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let response: Result<String, _> = client.request("ping", rpc_params![]).await;
        assert_eq!(response.unwrap(), "pong");
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_init() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_dir_path = temp_dir.path().to_str().unwrap();
        let params = RpcInitRequest {
            data_path: &format!("{}/data", temp_dir_path),
            cache_path: &format!("{}/cache", temp_dir_path),
            global_expire_time: 3600,
        };
        println!("{:?}", params);
        let response: Result<bool, _> = client.request("init", params).await;
        assert!(response.unwrap());
        handle.stop().unwrap();
    }
    #[tokio::test]
    async fn test_check_app_available() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/DUpdateSystem/UpgradeAll")
            .with_status(200)
            .create_async()
            .await;

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<bool, _> = client
            .check_app_available(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        assert!(response.unwrap());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_get_latest_release() {
        let body = fs::read_to_string("tests/files/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<ReleaseData, _> = client
            .get_latest_release(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        let release = response.unwrap();
        assert!(!release.version_number.is_empty());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_get_releases() {
        let body = fs::read_to_string("tests/files/web/github_api_release.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/DUpdateSystem/UpgradeAll/releases")
            .with_status(200)
            .with_body(body)
            .create();

        let id_map = BTreeMap::from([("owner", "DUpdateSystem"), ("repo", "UpgradeAll")]);
        let proxy_url = format!("{} -> {}", github::GITHUB_API_URL, server.url());
        let hub_data = BTreeMap::from([("reverse_proxy", proxy_url.as_str())]);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let params = RpcAppRequest {
            hub_uuid: "fd9b2602-62c5-4d55-bd1e-0d6537714ca0",
            app_data: id_map,
            hub_data,
        };
        println!("{:?}", params);
        let client = Client::new(url).unwrap();
        let response: Result<Vec<ReleaseData>, _> = client
            .get_releases(params.hub_uuid, params.app_data, params.hub_data)
            .await;
        let releases = response.unwrap();
        assert!(!releases.is_empty());
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_run_server_hanging() {
        let addr = "127.0.0.1:33334";
        let server_task = tokio::spawn(async move {
            // This should run the server and wait for the shutdown command
            run_server_hanging(addr, |url| {
                println!("Server started at {}", url);
                Ok(())
            })
            .await
            .expect("Server failed to run");
        });

        // Allow some time for the server to start up
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The callback should print the URL, but since we cannot capture that output easily in a test,
        // we assume the server starts correctly if no error happens till now.
        // Here, manually create a client and send a shutdown request
        let client = HttpClientBuilder::default()
            .build(format!("http://{}", addr))
            .expect("Failed to build client");

        let response: Result<(), _> = client.request("shutdown", rpc_params![]).await;
        assert!(response.is_ok(), "Failed to shutdown server");

        // Allow some time for the server to shut down
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Check if the shutdown was successful by confirming the server task is done
        if timeout(Duration::from_secs(1), server_task).await.is_err() {
            panic!("The server did not shut down within the expected time");
        }

        let response: Result<(), _> = client.request("shutdown", rpc_params![]).await;
        assert!(response.is_err(), "Server should not be running");
    }

    #[tokio::test]
    async fn test_get_cloud_config() {
        let body = fs::read_to_string("tests/files/web/cloud_config.json").unwrap();
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/cloud_config.json")
            .with_status(200)
            .with_body(body)
            .create();

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        println!("Server started at {}", url);
        let client = HttpClientBuilder::default().build(url).unwrap();
        let url = format!("{}/cloud_config.json", server.url());
        let params = RpcCloudConfigRequest { api_url: &url };
        println!("{:?}", params);
        let response: Result<ConfigList, _> = client.request("get_cloud_config", params).await;
        let config = response.unwrap();
        assert!(!config.app_config_list.is_empty());
        assert!(!config.hub_config_list.is_empty());
        handle.stop().unwrap();
    }

    // ========================================================================
    // WebSocket and message size tests
    // ========================================================================

    use jsonrpsee::ws_client::WsClientBuilder;
    use serial_test::serial;

    /// Generate a random ASCII string of given byte length (not compressible).
    /// Uses printable ASCII range (0x21-0x7e) to avoid JSON escape overhead.
    fn generate_random_string(size: usize) -> String {
        use rand::RngExt;
        let mut rng = rand::rng();
        let mut buf = vec![0u8; size];
        rng.fill(&mut buf[..]);
        // Map each byte to printable ASCII (0x21..=0x7e, 94 chars), avoid '"' and '\\'
        for b in buf.iter_mut() {
            *b = match (*b % 92) + 0x21 {
                b'"' => b'a',
                b'\\' => b'b',
                v => v,
            };
        }
        // SAFETY: all bytes are valid ASCII
        unsafe { String::from_utf8_unchecked(buf) }
    }

    /// Helper: start a minimal RPC server with "echo_data" method for size testing.
    async fn start_test_server_with_config(max_size: u32) -> (String, ServerHandle) {
        let config = ServerConfig::builder()
            .max_request_body_size(max_size)
            .max_response_body_size(max_size)
            .build();
        let server = jsonrpsee::server::Server::builder()
            .set_config(config)
            .build("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .unwrap();
        let mut module = RpcModule::new(());
        module
            .register_method("echo_data", |params, _, _| {
                let data: String = params.one()?;
                Ok::<String, ErrorObjectOwned>(data)
            })
            .unwrap();
        module.register_method("ping", |_, _, _| "pong").unwrap();
        let addr = server.local_addr().unwrap();
        let handle = server.start(module);
        (format!("ws://{}", addr), handle)
    }

    #[tokio::test]
    async fn test_ws_client_connection() {
        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        let ws_url = url.replace("http://", "ws://");
        let max_size = get_max_message_size();
        let client = WsClientBuilder::default()
            .max_request_size(max_size)
            .max_response_size(max_size)
            .build(&ws_url)
            .await
            .unwrap();
        let response: String = client.request("ping", rpc_params![]).await.unwrap();
        assert_eq!(response, "pong");
        handle.stop().unwrap();
    }

    #[tokio::test]
    async fn test_ws_large_message_50mb() {
        const SIZE: usize = 50 * 1024 * 1024; // 50MB
        let max_size = DEFAULT_MAX_SIZE;
        let (ws_url, handle) = start_test_server_with_config(max_size).await;
        let client = WsClientBuilder::default()
            .max_request_size(max_size)
            .max_response_size(max_size)
            .build(&ws_url)
            .await
            .unwrap();
        let data = generate_random_string(SIZE);
        let response: String = client
            .request("echo_data", rpc_params![&data])
            .await
            .unwrap();
        assert_eq!(response.len(), data.len());
        handle.stop().unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_ws_env_var_limits_message_size() {
        const ONE_MB: u32 = 1024 * 1024;
        const TWO_MB: usize = 2 * 1024 * 1024;

        // Set env var to 1MB limit
        // SAFETY: This test is marked #[serial] so no other tests run concurrently
        unsafe { std::env::set_var("GETTER_WS_MAX_MESSAGE_SIZE", ONE_MB.to_string()) };
        assert_eq!(get_max_message_size(), ONE_MB);

        let (url, handle) = run_server("", Arc::new(AtomicBool::new(true)))
            .await
            .unwrap();
        let ws_url = url.replace("http://", "ws://");

        // Client allows large messages, but server should reject
        let client = WsClientBuilder::default()
            .max_request_size(u32::MAX)
            .max_response_size(u32::MAX)
            .build(&ws_url)
            .await
            .unwrap();

        // Verify ping still works (small message)
        let response: String = client.request("ping", rpc_params![]).await.unwrap();
        assert_eq!(response, "pong");

        // Send 2MB data via init request, should be rejected by 1MB server limit
        let large_data = generate_random_string(TWO_MB);
        let params = RpcInitRequest {
            data_path: &large_data,
            cache_path: "/tmp/cache",
            global_expire_time: 3600,
        };
        let response: Result<bool, _> = client.request("init", params).await;
        assert!(
            response.is_err(),
            "2MB request should be rejected by 1MB server limit"
        );

        handle.stop().unwrap();
        // SAFETY: This test is marked #[serial] so no other tests run concurrently
        unsafe { std::env::remove_var("GETTER_WS_MAX_MESSAGE_SIZE") };
    }
}
