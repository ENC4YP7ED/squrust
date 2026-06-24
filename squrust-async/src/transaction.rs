//! Explicit transactions.

use std::marker::PhantomData;
use std::sync::Arc;

use squrust_core::WriteTx;
use squrust_serde::{FromRow, ToParams};
use squrust_sql::{ReadSource, SqlEngine};

use crate::connection::SqurustConnection;
use crate::error::Result;
use crate::query::column_names;
use crate::row::SqurustRow;

/// A read-write transaction. Reads observe the transaction's own uncommitted
/// writes. Dropping without [`commit`](Transaction::commit) rolls back.
pub struct Transaction<'a> {
    engine: Arc<SqlEngine>,
    tx: Arc<WriteTx>,
    done: bool,
    _marker: PhantomData<&'a SqurustConnection>,
}

impl<'a> Transaction<'a> {
    pub(crate) fn new(conn: &'a SqurustConnection) -> Self {
        let engine = conn.engine().clone();
        let tx = Arc::new(engine.storage().begin_write());
        Transaction {
            engine,
            tx,
            done: false,
            _marker: PhantomData,
        }
    }

    /// Run a DML statement (INSERT/UPDATE/DELETE) inside the transaction.
    pub async fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64> {
        self.engine
            .execute_on(&self.tx, sql, &params.to_params())
            .await
            .map_err(Into::into)
    }

    /// Run a SELECT inside the transaction, reading the transaction's own state.
    pub async fn fetch_all<T: FromRow>(
        &self,
        sql: &str,
        params: impl ToParams,
    ) -> Result<Vec<T>> {
        let source: ReadSource = self.tx.clone();
        let mut exec = self.engine.build_query(source, sql, &params.to_params())?;
        let names = column_names(&*exec);
        let mut out = Vec::new();
        while let Some(row) = exec.next().await? {
            let srow = SqurustRow::new(row, names.clone());
            out.push(T::from_row(&srow)?);
        }
        Ok(out)
    }

    pub async fn commit(mut self) -> Result<()> {
        self.tx.commit()?;
        self.done = true;
        Ok(())
    }

    pub async fn rollback(mut self) {
        self.tx.rollback();
        self.done = true;
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.tx.rollback();
        }
    }
}
