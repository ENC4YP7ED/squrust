//! Error types for the storage engine.

use std::io;

/// Errors produced by the storage engine.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("not a squrust/sqlite database file: bad magic header")]
    BadMagic,

    #[error("unsupported page size: {0}")]
    BadPageSize(u32),

    #[error("page {0} is out of range")]
    PageOutOfRange(u32),

    #[error("WAL frame is corrupt: {0}")]
    CorruptWal(String),

    #[error("b-tree corruption: {0}")]
    Corrupt(String),

    #[error("transaction conflict")]
    Conflict,

    #[error("database is read-only")]
    ReadOnly,
}

pub type Result<T> = std::result::Result<T, StorageError>;
