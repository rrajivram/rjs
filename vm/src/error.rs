use thiserror::Error;

#[derive(Debug, Error)]
pub enum JSError {
    #[error("Cannot access {0} before initialization")]
    RefereneError(String),
    #[error("Cannot reassign const value {0}")]
    InvalidReassign(String),
    #[error("Errors compiling : {0:?}")]
    BadJavascript(Vec<String>),
    #[error("Object not found : {0}")]
    ObjectNotFound(String),
    #[error("Unknown Error")]
    UnknownError,
}
