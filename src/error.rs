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

pub type Result<T> = result::Result<T, GetterError>;
