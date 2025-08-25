pub mod server;
pub mod client;
pub mod types;

pub use server::GetterRpcServer;
pub use client::{GetterRpcClient, RpcError};
pub use types::*;