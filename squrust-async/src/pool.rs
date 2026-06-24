//! A lightweight connection pool.
//!
//! All pooled handles share a single underlying engine (the storage engine
//! already supports concurrent readers plus a single writer). A semaphore
//! bounds the number of handles checked out at once.

use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use squrust_core::StorageEngine;
use squrust_sql::SqlEngine;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::connection::SqurustConnection;
use crate::error::{Result, SqurustError};

/// A pool of connection handles over one shared database.
#[derive(Clone)]
pub struct SqurustPool {
    conn: SqurustConnection,
    sem: Arc<Semaphore>,
}

impl SqurustPool {
    pub async fn new(path: impl AsRef<Path>, max_size: usize) -> Result<Self> {
        let storage = StorageEngine::open(path.as_ref())?;
        let engine = SqlEngine::new(storage).await?;
        Ok(SqurustPool {
            conn: SqurustConnection::from_engine(engine),
            sem: Arc::new(Semaphore::new(max_size.max(1))),
        })
    }

    pub async fn open_memory(max_size: usize) -> Result<Self> {
        let storage = StorageEngine::open_memory()?;
        let engine = SqlEngine::new(storage).await?;
        Ok(SqurustPool {
            conn: SqurustConnection::from_engine(engine),
            sem: Arc::new(Semaphore::new(max_size.max(1))),
        })
    }

    /// Check out a connection, waiting if the pool is at capacity.
    pub async fn get(&self) -> Result<PooledConnection> {
        let permit = self
            .sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SqurustError::PoolClosed)?;
        Ok(PooledConnection {
            conn: self.conn.clone(),
            _permit: permit,
        })
    }

    pub fn close(&self) {
        self.sem.close();
    }
}

/// A connection checked out from a [`SqurustPool`]. Returns its permit to the
/// pool when dropped.
pub struct PooledConnection {
    conn: SqurustConnection,
    _permit: OwnedSemaphorePermit,
}

impl Deref for PooledConnection {
    type Target = SqurustConnection;
    fn deref(&self) -> &SqurustConnection {
        &self.conn
    }
}
