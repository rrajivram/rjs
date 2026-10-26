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
    #[error("No such method : {0}")]
    MethodNotFound(String),
    #[error("No such property : {0}")]
    ReferenceError(String),
    #[error("TypeError: {0}")]
    TypeError(String),
    #[error("RangeError: {0}")]
    RangeError(String),
    #[error("Not supported yet: {0}")]
    NotSupported(String),
    #[error("Unknown Error {0}")]
    UnknownError(String),
}
