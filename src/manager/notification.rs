use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::database::models::app::AppRecord;
use crate::manager::app_status::AppStatus;

/// Global notification dispatcher, registered via `register_notification` RPC.
static NOTIFICATION: OnceCell<NotificationDispatcher> = OnceCell::new();

pub fn set_notification(url: String) {
    let _ = NOTIFICATION.set(NotificationDispatcher { url });
}

pub fn get_notification() -> Option<&'static NotificationDispatcher> {
    NOTIFICATION.get()
}

/// Events emitted by the Rust manager layer to the Kotlin UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManagerEvent {
    AppStatusChanged {
        record_id: String,
        old_status: AppStatus,
        new_status: AppStatus,
    },
    RenewProgress {
        done: usize,
        total: usize,
    },
    AppAdded {
        record: AppRecord,
    },
    AppDeleted {
        record_id: String,
    },
    AppDatabaseChanged {
        record: AppRecord,
    },
}

/// Dispatches manager events to the Kotlin UI layer via HTTP JSON-RPC.
///
/// Kotlin registers a notification URL via `register_notification` RPC.
/// When an event fires, Rust POSTs `on_manager_event({event})` to that URL.
/// The Kotlin server handles it and updates the UI (ViewModel / LiveData).
pub struct NotificationDispatcher {
    url: String,
}

impl NotificationDispatcher {
    /// Fire an event notification to Kotlin. Best-effort: errors are logged but not propagated.
    pub async fn notify(&self, event: ManagerEvent) {
        let body = match build_jsonrpc_request("on_manager_event", &event) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("NotificationDispatcher: failed to serialize event: {e}");
                return;
            }
        };

        let client = reqwest::Client::new();
        match client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(_) => {}
            Err(e) => {
                eprintln!("NotificationDispatcher: failed to send notification: {e}");
            }
        }
    }
}

fn build_jsonrpc_request<T: Serialize>(
    method: &str,
    params: &T,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct JsonRpcRequest<'a, P: Serialize> {
        jsonrpc: &'a str,
        method: &'a str,
        params: &'a P,
        id: u64,
    }
    serde_json::to_string(&JsonRpcRequest {
        jsonrpc: "2.0",
        method,
        params,
        id: 1,
    })
}

/// Convenience: notify if a dispatcher is registered (no-op otherwise).
pub async fn notify_if_registered(event: ManagerEvent) {
    if let Some(dispatcher) = get_notification() {
        dispatcher.notify(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serialization_status_changed() {
        let event = ManagerEvent::AppStatusChanged {
            record_id: "abc-123".to_string(),
            old_status: AppStatus::AppPending,
            new_status: AppStatus::AppOutdated,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("app_status_changed"));
        assert!(json.contains("abc-123"));
        assert!(json.contains("app_outdated"));
    }

    #[test]
    fn test_event_serialization_renew_progress() {
        let event = ManagerEvent::RenewProgress { done: 3, total: 10 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("renew_progress"));
        assert!(json.contains("10"));
    }

    #[test]
    fn test_event_serialization_app_deleted() {
        let event = ManagerEvent::AppDeleted {
            record_id: "del-456".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("app_deleted"));
        assert!(json.contains("del-456"));
    }

    #[test]
    fn test_jsonrpc_request_format() {
        let event = ManagerEvent::RenewProgress { done: 1, total: 5 };
        let body = build_jsonrpc_request("on_manager_event", &event).unwrap();
        assert!(body.contains("\"jsonrpc\":\"2.0\""));
        assert!(body.contains("\"method\":\"on_manager_event\""));
        assert!(body.contains("renew_progress"));
    }
}
