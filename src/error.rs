use std::result;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Other(String),
    Getter(GetterError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {e}"),
            Error::Other(msg) => write!(f, "{msg}"),
            Error::Getter(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Getter(e) => e.source(),
            Error::Other(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<GetterError> for Error {
    fn from(e: GetterError) -> Self {
        Error::Getter(e)
    }
}

pub type Result<T> = result::Result<T, Error>;

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
