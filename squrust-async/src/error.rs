//! The top-level error type wrapping all lower-level errors.

use squrust_core::StorageError;
use squrust_sql::SqlError;

#[derive(Debug, thiserror::Error)]
pub enum SqurustError {
    #[error(transparent)]
    Sql(#[from] SqlError),

    #[error(transparent)]
    Storage(StorageError),

    #[error("no rows returned")]
    RowNotFound,

    #[error("type conversion error: {0}")]
    Conversion(String),

    #[error("pool exhausted or closed")]
    PoolClosed,
}

impl From<StorageError> for SqurustError {
    fn from(e: StorageError) -> Self {
        SqurustError::Storage(e)
    }
}

pub type Result<T> = std::result::Result<T, SqurustError>;
