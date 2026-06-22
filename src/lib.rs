//! Root getter library facade for the UpgradeAll rewrite.
//!
//! The root crate is intentionally small during the rewrite. Product/domain
//! behavior lives in split crates such as `getter-core` and `getter-storage`;
//! Android/Flutter hosts embed this crate through a stable facade.

#[cfg(feature = "domain")]
pub use getter_core as core;
#[cfg(feature = "domain")]
pub use getter_storage as storage;

#[cfg(feature = "rustls-platform-verifier")]
pub use rustls_platform_verifier;

pub mod rpc {
    pub mod server {
        use std::future::pending;

        /// Error returned by the temporary in-process RPC server facade.
        #[derive(Debug, thiserror::Error)]
        pub enum RpcServerError {
            #[error("failed to bind RPC server at {address}: {source}")]
            Bind {
                address: String,
                #[source]
                source: std::io::Error,
            },
            #[error("failed to read local RPC server address: {0}")]
            LocalAddress(#[source] std::io::Error),
            #[error("RPC server startup callback failed")]
            StartupCallback,
        }

        /// Start a placeholder local RPC endpoint and keep it alive.
        ///
        /// This preserves the Android `api_proxy` dependency target while the
        /// full getter RPC surface is rewritten. The function binds a real local
        /// TCP listener so callers receive a concrete URL, then parks forever.
        pub async fn run_server_hanging<F>(
            address: &str,
            on_listening: F,
        ) -> Result<(), RpcServerError>
        where
            F: FnOnce(&str) -> Result<(), RpcServerError> + Send + 'static,
        {
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .map_err(|source| RpcServerError::Bind {
                    address: address.to_owned(),
                    source,
                })?;
            let local_addr = listener
                .local_addr()
                .map_err(RpcServerError::LocalAddress)?;
            let url = format!("ws://{local_addr}");
            on_listening(&url)?;

            // Keep the listener alive until the host process stops. The full
            // JSON-RPC implementation is added in a later behavior slice.
            let _listener = listener;
            pending::<()>().await;
            #[allow(unreachable_code)]
            Ok(())
        }
    }
}
