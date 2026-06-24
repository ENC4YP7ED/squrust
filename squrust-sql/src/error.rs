//! SQL-layer errors.

use squrust_core::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("schema error: {0}")]
    Schema(String),

    #[error("type error: {0}")]
    Type(String),

    #[error("constraint violation: {0}")]
    Constraint(String),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("ambiguous: {0}")]
    Ambiguous(String),

    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, SqlError>;
