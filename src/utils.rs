use bytes::Bytes;
use hyper::{Client, StatusCode, Uri};
use std::fmt;

// Custom http response Error
#[derive(Debug)]
pub struct ResponseError {
    pub status: u16,
    pub body: Bytes,
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ResponseError status: {}, body: {}",
            self.status,
            String::from_utf8_lossy(&self.body)
        )
    }
}
impl std::error::Error for ResponseError {}

pub async fn http_get(url: Uri) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();

    let res = client.get(url).await?;
    let status = res.status();
    let body = hyper::body::to_bytes(res.into_body()).await?;
    if status == StatusCode::OK {
        Ok(body)
    } else {
        Err(Box::new(ResponseError {
            status: status.as_u16(),
            body,
        }))
    }
}

pub async fn https_get(url: Uri) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_only()
        .enable_http1()
        .build();

    let client: Client<_, hyper::Body> = Client::builder().build(https);

    let res = client.get(url).await?;
    let status = res.status();
    let body = hyper::body::to_bytes(res.into_body()).await?;
    if status == StatusCode::OK {
        Ok(body)
    } else {
        Err(Box::new(ResponseError {
            status: status.as_u16(),
            body,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_https_get() {
        let url = "https://example.com".parse().unwrap();
        let result = https_get(url).await;
        assert!(result.is_ok());
        assert!(result.unwrap().len() > 0);
    }

    #[tokio::test]
    async fn test_https_get_invalid() {
        let url = "https://123123".parse().unwrap();
        let result = https_get(url).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_https_get_status() {
        let url = "https://httpbin.org/status/404".parse().unwrap();
        let result = https_get(url).await;

        if let Err(error) = result {
            assert_eq!(error.downcast_ref::<ResponseError>().unwrap().status, 404);
        } else {
            panic!("Should return error.");
        }
    }

    #[tokio::test]
    async fn test_http_get() {
        let url = "http://example.com".parse().unwrap();
        let result = http_get(url).await;
        assert!(result.is_ok());
        assert!(result.unwrap().len() > 0);
    }
}
