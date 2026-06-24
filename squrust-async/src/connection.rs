//! The connection handle.

use std::path::Path;
use std::sync::Arc;

use squrust_core::StorageEngine;
use squrust_serde::ToParams;
use squrust_sql::SqlEngine;

use crate::error::Result;
use crate::migrate::{Migration, run_migrations};
use crate::query::Query;
use crate::transaction::Transaction;

/// A handle to a Squrust database. Cloning shares the same underlying engine.
#[derive(Clone)]
pub struct SqurustConnection {
    inner: Arc<SqlEngine>,
}

impl SqurustConnection {
    /// Open (creating if missing) a database at `path`.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let storage = StorageEngine::open(path.as_ref())?;
        let inner = SqlEngine::new(storage).await?;
        Ok(SqurustConnection { inner })
    }

    /// Open a transient in-memory database.
    pub async fn open_memory() -> Result<Self> {
        let storage = StorageEngine::open_memory()?;
        let inner = SqlEngine::new(storage).await?;
        Ok(SqurustConnection { inner })
    }

    pub(crate) fn from_engine(inner: Arc<SqlEngine>) -> Self {
        SqurustConnection { inner }
    }

    pub(crate) fn engine(&self) -> &Arc<SqlEngine> {
        &self.inner
    }

    /// Start a query builder.
    pub fn query<'a>(&'a self, sql: &str) -> Query<'a> {
        Query::new(self, sql)
    }

    /// Execute a statement with parameters, returning rows affected.
    pub async fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64> {
        self.inner
            .execute(sql, &params.to_params())
            .await
            .map_err(Into::into)
    }

    /// Begin a transaction.
    pub async fn begin(&self) -> Result<Transaction<'_>> {
        Ok(Transaction::new(self))
    }

    /// Run a SELECT and return its column names together with each row's raw
    /// values. Useful for dynamic consumers like the CLI.
    pub async fn fetch_raw(
        &self,
        sql: &str,
        params: impl ToParams,
    ) -> Result<(Vec<String>, Vec<Vec<squrust_sql::Value>>)> {
        let p = params.to_params();
        let mut exec = self.inner.query(sql, &p).await?;
        let names: Vec<String> = exec.columns().iter().map(|c| c.name.clone()).collect();
        let mut rows = Vec::new();
        while let Some(row) = exec.next().await? {
            rows.push(row.values);
        }
        Ok((names, rows))
    }

    /// Run pending migrations in order.
    pub async fn migrate(&self, migrations: &[Migration]) -> Result<()> {
        run_migrations(self, migrations).await
    }

    /// Fold the WAL into the main database file.
    pub fn checkpoint(&self) -> Result<()> {
        self.inner.storage().checkpoint().map_err(Into::into)
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.inner.last_insert_rowid()
    }

    pub fn changes(&self) -> i64 {
        self.inner.changes()
    }

    /// User table names.
    pub fn table_names(&self) -> Vec<String> {
        self.inner.table_names()
    }

    /// `CREATE TABLE` text for all tables (or a single one when `table` is set).
    pub fn schema(&self, table: Option<&str>) -> Vec<String> {
        match table {
            Some(t) => self.inner.table_sql(t).into_iter().collect(),
            None => self.inner.schema_statements(),
        }
    }
}
