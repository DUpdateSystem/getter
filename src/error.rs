use std::result;

#[derive(Debug)]
pub enum Error {
    Custom(String),
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(String),
    Network(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Custom(msg) => write!(f, "Custom error: {}", msg),
            Error::Io(err) => write!(f, "IO error: {}", err),
            Error::Json(err) => write!(f, "JSON error: {}", err),
            Error::Http(msg) => write!(f, "HTTP error: {}", msg),
            Error::Network(msg) => write!(f, "Network error: {}", msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Json(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Json(err)
    }
}

impl From<GetterError> for Error {
    fn from(err: GetterError) -> Self {
        Error::Custom(format!("{}: {}", err.tag, err.message))
    }
}

pub type Result<T> = result::Result<T, Error>;

// Legacy error type for backward compatibility
#[derive(Debug)]
pub struct GetterError {
    pub tag: String,
    pub message: String,
    pub err: Option<Box<dyn std::error::Error>>,
}

impl GetterError {
    pub fn new_nobase(tag: &str, message: &str) -> Self {
        Self {
            tag: tag.to_string(),
            message: message.to_string(),
            err: None,
        }
    }

    pub fn new(tag: &str, message: &str, err: Box<dyn std::error::Error>) -> Self {
        Self {
            tag: tag.to_string(),
            message: message.to_string(),
            err: Some(err),
        }
    }
}

impl std::fmt::Display for GetterError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {}", self.tag, self.message)
    }
}

impl std::error::Error for GetterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.err {
            Some(err) => Some(err.as_ref()),
            None => None,
        }
    }
}
