use crate::types::*;
use getter_appmanager::{get_app_manager, AppManager};
use hyper::{body::Bytes, service::Service, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use tokio::net::TcpListener;

type Body = http_body_util::Full<Bytes>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub struct GetterRpcServer {
    app_manager: &'static AppManager,
}

impl Default for GetterRpcServer {
    fn default() -> Self {
        Self::new()
    }
}

impl GetterRpcServer {
    pub fn new() -> Self {
        Self {
            app_manager: get_app_manager(),
        }
    }

    pub async fn start(self, addr: SocketAddr) -> Result<(), BoxError> {
        let listener = TcpListener::bind(addr).await?;
        println!("RPC Server listening on {}", addr);

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let service = RpcService {
                app_manager: self.app_manager,
            };

            tokio::spawn(async move {
                if let Err(err) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                {
                    eprintln!("Error serving connection: {:?}", err);
                }
            });
        }
    }
}

#[derive(Clone)]
struct RpcService {
    app_manager: &'static AppManager,
}

impl Service<Request<hyper::body::Incoming>> for RpcService {
    type Response = Response<Body>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<hyper::body::Incoming>) -> Self::Future {
        let app_manager = self.app_manager;

        Box::pin(async move {
            if req.method() != Method::POST {
                return Ok(Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .body(Body::from("Method not allowed"))?);
            }

            let body_bytes = match http_body_util::BodyExt::collect(req.into_body()).await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from(format!("Failed to read body: {}", e)))?);
                }
            };

            let rpc_request: RpcRequest = match serde_json::from_slice(&body_bytes) {
                Ok(req) => req,
                Err(e) => {
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from(format!("Invalid JSON-RPC request: {}", e)))?);
                }
            };

            let response = handle_rpc_request(app_manager, rpc_request).await;
            let response_json = serde_json::to_string(&response)?;

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(response_json))?)
        })
    }
}

async fn handle_rpc_request(app_manager: &'static AppManager, request: RpcRequest) -> RpcResponse {
    match request.method.as_str() {
        "add_app" => {
            if let Some(params) = request.params {
                if let Ok(add_req) = serde_json::from_value::<AddAppRequest>(params) {
                    match app_manager
                        .add_app(
                            add_req.app_id,
                            add_req.hub_uuid,
                            add_req.app_data,
                            add_req.hub_data,
                        )
                        .await
                    {
                        Ok(msg) => {
                            RpcResponse::success(request.id, serde_json::json!({"message": msg}))
                        }
                        Err(e) => RpcResponse::error(request.id, -1, e),
                    }
                } else {
                    RpcResponse::error(request.id, -32602, "Invalid parameters".to_string())
                }
            } else {
                RpcResponse::error(request.id, -32602, "Missing parameters".to_string())
            }
        }
        "remove_app" => {
            if let Some(params) = request.params {
                if let Ok(remove_req) = serde_json::from_value::<RemoveAppRequest>(params) {
                    match app_manager.remove_app(&remove_req.app_id).await {
                        Ok(success) => RpcResponse::success(
                            request.id,
                            serde_json::json!({"removed": success}),
                        ),
                        Err(e) => RpcResponse::error(request.id, -1, e),
                    }
                } else {
                    RpcResponse::error(request.id, -32602, "Invalid parameters".to_string())
                }
            } else {
                RpcResponse::error(request.id, -32602, "Missing parameters".to_string())
            }
        }
        "list_apps" => match app_manager.list_apps().await {
            Ok(apps) => RpcResponse::success(request.id, serde_json::to_value(apps).unwrap()),
            Err(e) => RpcResponse::error(request.id, -1, e),
        },
        _ => RpcResponse::error(request.id, -32601, "Method not found".to_string()),
    }
}
