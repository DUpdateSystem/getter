use std::result;

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

pub type Result<T> = result::Result<T, GetterError>;
