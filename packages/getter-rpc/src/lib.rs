pub mod client;
pub mod server;
pub mod types;

pub use client::{GetterRpcClient, RpcError};
pub use server::GetterRpcServer;
pub use types::*;
