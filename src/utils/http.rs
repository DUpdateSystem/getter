use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Empty};
use hyper::{StatusCode, Uri};
#[cfg(not(feature = "rustls-platform-verifier"))]
use hyper_rustls::ConfigBuilderExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use once_cell::sync::Lazy;
use rustls::ClientConfig;
#[cfg(feature = "rustls-platform-verifier")]
use rustls_platform_verifier::BuilderVerifierExt;
use std::{collections::HashMap, fmt};

// Custom http response Error
#[derive(Debug)]
pub struct ResponseData {
    pub status: u16,
    pub body: Option<Bytes>,
}

impl fmt::Display for ResponseData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Response status: {}, body: {}",
            self.status,
            self.body.as_ref().map_or_else(
                || "".to_string(),
                |body| String::from_utf8_lossy(body).to_string(),
            )
        )
    }
}

pub async fn get(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    if url.scheme_str() == Some("https") {
        https_get(url, header_map).await
    } else {
        http_get(url, header_map).await
    }
}

pub async fn head(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    if url.scheme_str() == Some("https") {
        https_head(url, header_map).await
    } else {
        http_head(url, header_map).await
    }
}

pub async fn http_get(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    _http_get(url, header_map, false).await
}

pub async fn http_head(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    _http_get(url, header_map, true).await
}

async fn _http_get(
    url: Uri,
    header_map: &HashMap<String, String>,
    only_status: bool,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    let http = HttpConnector::new();
    let client = Client::builder(TokioExecutor::new()).build(http);

    let mut req = hyper::Request::builder().method("GET").uri(url.clone());
    for (key, value) in header_map {
        req = req.header(key, value);
    }
    let req = req.body(Empty::<Bytes>::new())?;
    let mut res = client.request(req).await?;
    let status = res.status();
    if only_status {
        Ok(ResponseData {
            status: status.as_u16(),
            body: None,
        })
    } else {
        let mut body = BytesMut::new();
        while let Some(next) = res.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                body.extend_from_slice(chunk);
            }
        }
        Ok(ResponseData {
            status: status.as_u16(),
            body: Some(body.freeze()),
        })
    }
}

pub async fn https_get(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    _https_get(url, header_map, false).await
}

pub async fn https_head(
    url: Uri,
    header_map: &HashMap<String, String>,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    _https_get(url, header_map, true).await
}

// Global https provider with lazy initialization
static PROVIDER: Lazy<std::sync::Arc<rustls::crypto::CryptoProvider>> =
    Lazy::new(|| std::sync::Arc::new(rustls::crypto::ring::default_provider()));

// Https config error wrapper error
struct HttpsConfigError {
    error: Box<dyn std::error::Error + Send + Sync>,
}
impl fmt::Display for HttpsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HttpsConfigError: {}", self.error)
    }
}
impl fmt::Debug for HttpsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HttpsConfigError: {:?}", self.error)
    }
}
impl std::error::Error for HttpsConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

fn https_config() -> Result<hyper_rustls::HttpsConnector<HttpConnector>, HttpsConfigError> {
    let provider = PROVIDER.clone();
    let tls: rustls::ClientConfig;
    #[cfg(feature = "rustls-platform-verifier")]
    {
        tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| HttpsConfigError { error: Box::new(e) })?
            .with_platform_verifier()
            .with_no_client_auth();
    }
    #[cfg(all(feature = "webpki-roots", not(feature = "rustls-platform-verifier")))]
    {
        tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| HttpsConfigError { error: Box::new(e) })?
            .with_webpki_roots()
            .with_no_client_auth();
    }
    #[cfg(all(
        feature = "native-tokio",
        not(feature = "webpki-roots"),
        not(feature = "rustls-platform-verifier")
    ))]
    {
        tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| HttpsConfigError { error: Box::new(e) })?
            .with_native_roots()
            .map_err(|e| HttpsConfigError { error: Box::new(e) })?
            .with_no_client_auth();
    }
    #[cfg(all(
        not(feature = "native-tokio"),
        not(feature = "webpki-roots"),
        not(feature = "rustls-platform-verifier")
    ))]
    {
        compile_error!("No TLS backend enabled");
    }
    Ok(hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build())
}

async fn _https_get(
    url: Uri,
    header_map: &HashMap<String, String>,
    only_status: bool,
) -> Result<ResponseData, Box<dyn std::error::Error + Send + Sync>> {
    let https = https_config()?;
    let client = Client::builder(TokioExecutor::new()).build(https);
    let mut req = hyper::Request::builder().method("GET").uri(url.clone());
    for (key, value) in header_map {
        req = req.header(key, value);
    }
    let req = req.body(Empty::<Bytes>::new())?;

    let mut res = client.request(req).await?;
    let status = res.status();
    if only_status {
        Ok(ResponseData {
            status: status.as_u16(),
            body: None,
        })
    } else {
        let mut body = BytesMut::new();
        while let Some(next) = res.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                body.extend_from_slice(chunk);
            }
        }
        Ok(ResponseData {
            status: status.as_u16(),
            body: Some(body.freeze()),
        })
    }
}

pub fn http_status_is_ok(status: u16) -> bool {
    if let Ok(status) = StatusCode::from_u16(status) {
        !(status.is_client_error() || status.is_server_error())
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_https_get() {
        let url = "https://example.com".parse().unwrap();
        let result = https_get(url, &HashMap::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().body.unwrap().len() > 0);
    }

    #[tokio::test]
    async fn test_https_get_invalid() {
        let url = "https://123123".parse().unwrap();
        let result = https_get(url, &HashMap::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_https_get_status() {
        let url = "https://httpstat.us/418".parse().unwrap();
        let result = https_get(url, &HashMap::new()).await;
        assert_eq!(result.unwrap().status, 418);
    }

    #[tokio::test]
    async fn test_https_head() {
        let url = "https://example.com".parse().unwrap();
        let result = https_head(url, &HashMap::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().body.is_none());
    }

    #[tokio::test]
    async fn test_https_get_header() {
        let url = "https://postman-echo.com/get".parse().unwrap();
        let header_map = {
            let mut map = HashMap::new();
            map.insert("X-Test".to_string(), "test000".to_string());
            map.insert("Test-Header".to_string(), "test001".to_string());
            map
        };
        let result = https_get(url, &header_map).await;
        assert!(result.is_ok());
        let body = result.unwrap().body.expect("Response body was empty");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("Failed to parse JSON");
        for (key, value) in header_map {
            // key to lowercase, for fix podman-echo
            // Due https://stackoverflow.com/questions/5258977/are-http-headers-case-sensitive
            let key = key.to_lowercase();
            assert_eq!(json["headers"][key], value);
        }
    }

    #[tokio::test]
    async fn test_http_get() {
        let url = "http://example.com".parse().unwrap();
        let result = http_get(url, &HashMap::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().body.unwrap().len() > 0);
    }

    #[tokio::test]
    async fn test_http_head() {
        let url = "http://example.com".parse().unwrap();
        let result = http_head(url, &HashMap::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().body.is_none());
    }

    #[tokio::test]
    async fn test_http_get_header() {
        let url = "http://postman-echo.com/get".parse().unwrap();
        let header_map = {
            let mut map = HashMap::new();
            map.insert("X-Test".to_string(), "test000".to_string());
            map.insert("Test-Header".to_string(), "test001".to_string());
            map
        };
        let result = http_get(url, &header_map).await;
        assert!(result.is_ok());
        let body = result.unwrap().body.expect("Response body was empty");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("Failed to parse JSON");
        for (key, value) in header_map {
            let key = key.to_lowercase();
            assert_eq!(json["headers"][key], value);
        }
    }
}
