//! # squrust-sync
//!
//! A blocking wrapper around [`squrust_async`]. Each call drives the async API
//! to completion on a runtime: a private current-thread runtime when called
//! outside any reactor, or the ambient runtime via `block_in_place` when called
//! from within one.

#![forbid(unsafe_code)]

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use squrust_async::{
    Migration, PooledConnection, Result, SqurustConnection, SqurustPool, ToParams, Transaction,
    Value,
};
use squrust_core::StorageError;
use tokio::runtime::{Builder, Handle, Runtime};

// Re-export the data types users need.
pub use squrust_async::{FromRow, SqurustError, SqurustRow};

fn new_runtime() -> Result<Arc<Runtime>> {
    let rt = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| SqurustError::from(StorageError::Io(e)))?;
    Ok(Arc::new(rt))
}

/// Drive a future to completion synchronously.
fn block_on<F: Future>(rt: &Runtime, fut: F) -> F::Output {
    match Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(move || handle.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

/// A blocking database connection.
pub struct SyncConnection {
    conn: SqurustConnection,
    rt: Arc<Runtime>,
}

impl SyncConnection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let rt = new_runtime()?;
        let conn = block_on(&rt, SqurustConnection::open(path))?;
        Ok(SyncConnection { conn, rt })
    }

    pub fn open_memory() -> Result<Self> {
        let rt = new_runtime()?;
        let conn = block_on(&rt, SqurustConnection::open_memory())?;
        Ok(SyncConnection { conn, rt })
    }

    pub fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64> {
        block_on(&self.rt, self.conn.execute(sql, params))
    }

    pub fn query<'a>(&'a self, sql: &str) -> SyncQuery<'a> {
        SyncQuery {
            conn: self,
            sql: sql.to_string(),
            params: Vec::new(),
        }
    }

    pub fn begin(&self) -> Result<SyncTransaction<'_>> {
        let txn = block_on(&self.rt, self.conn.begin())?;
        Ok(SyncTransaction {
            txn,
            rt: self.rt.clone(),
        })
    }

    pub fn migrate(&self, migrations: &[Migration]) -> Result<()> {
        block_on(&self.rt, self.conn.migrate(migrations))
    }

    pub fn checkpoint(&self) -> Result<()> {
        self.conn.checkpoint()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }
}

/// A blocking query builder.
pub struct SyncQuery<'a> {
    conn: &'a SyncConnection,
    sql: String,
    params: Vec<Value>,
}

impl SyncQuery<'_> {
    pub fn bind(mut self, value: impl Into<Value>) -> Self {
        self.params.push(value.into());
        self
    }

    pub fn fetch_all<T: FromRow>(self) -> Result<Vec<T>> {
        let SyncQuery { conn, sql, params } = self;
        block_on(&conn.rt, async move {
            build(&conn.conn, &sql, params).fetch_all::<T>().await
        })
    }

    pub fn fetch_one<T: FromRow>(self) -> Result<T> {
        let SyncQuery { conn, sql, params } = self;
        block_on(&conn.rt, async move {
            build(&conn.conn, &sql, params).fetch_one::<T>().await
        })
    }

    pub fn fetch_optional<T: FromRow>(self) -> Result<Option<T>> {
        let SyncQuery { conn, sql, params } = self;
        block_on(&conn.rt, async move {
            build(&conn.conn, &sql, params).fetch_optional::<T>().await
        })
    }

    pub fn execute(self) -> Result<u64> {
        let SyncQuery { conn, sql, params } = self;
        block_on(&conn.rt, async move {
            build(&conn.conn, &sql, params).execute().await
        })
    }
}

fn build<'a>(
    conn: &'a SqurustConnection,
    sql: &str,
    params: Vec<Value>,
) -> squrust_async::Query<'a> {
    let mut q = conn.query(sql);
    for v in params {
        q = q.bind(v);
    }
    q
}

/// A blocking transaction.
pub struct SyncTransaction<'a> {
    txn: Transaction<'a>,
    rt: Arc<Runtime>,
}

impl SyncTransaction<'_> {
    pub fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64> {
        block_on(&self.rt, self.txn.execute(sql, params))
    }

    pub fn fetch_all<T: FromRow>(&self, sql: &str, params: impl ToParams) -> Result<Vec<T>> {
        block_on(&self.rt, self.txn.fetch_all::<T>(sql, params))
    }

    pub fn commit(self) -> Result<()> {
        let SyncTransaction { txn, rt } = self;
        block_on(&rt, txn.commit())
    }

    pub fn rollback(self) {
        let SyncTransaction { txn, rt } = self;
        block_on(&rt, txn.rollback());
    }
}

/// A blocking connection pool.
pub struct SyncPool {
    pool: SqurustPool,
    rt: Arc<Runtime>,
}

impl SyncPool {
    pub fn new(path: impl AsRef<Path>, max_size: usize) -> Result<Self> {
        let rt = new_runtime()?;
        let pool = block_on(&rt, SqurustPool::new(path, max_size))?;
        Ok(SyncPool { pool, rt })
    }

    pub fn open_memory(max_size: usize) -> Result<Self> {
        let rt = new_runtime()?;
        let pool = block_on(&rt, SqurustPool::open_memory(max_size))?;
        Ok(SyncPool { pool, rt })
    }

    pub fn get(&self) -> Result<SyncPooledConnection> {
        let inner = block_on(&self.rt, self.pool.get())?;
        Ok(SyncPooledConnection {
            inner,
            rt: self.rt.clone(),
        })
    }
}

/// A blocking handle checked out from a [`SyncPool`].
pub struct SyncPooledConnection {
    inner: PooledConnection,
    rt: Arc<Runtime>,
}

impl SyncPooledConnection {
    pub fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64> {
        block_on(&self.rt, self.inner.execute(sql, params))
    }

    pub fn fetch_all<T: FromRow>(&self, sql: &str, params: impl ToParams) -> Result<Vec<T>> {
        let conn: &SqurustConnection = &self.inner;
        block_on(&self.rt, async move {
            build(conn, sql, params.to_params()).fetch_all::<T>().await
        })
    }
}
