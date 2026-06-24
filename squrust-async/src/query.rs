//! The query builder.

use std::sync::Arc;

use squrust_serde::FromRow;
use squrust_sql::{Executor, Value};

use crate::connection::SqurustConnection;
use crate::error::{Result, SqurustError};
use crate::row::SqurustRow;
use crate::stream::RowStream;

/// A query builder. Bind parameters with [`bind`](Query::bind), then run one of
/// the `fetch_*` methods or [`execute`](Query::execute).
pub struct Query<'a> {
    conn: &'a SqurustConnection,
    sql: String,
    params: Vec<Value>,
}

impl<'a> Query<'a> {
    pub(crate) fn new(conn: &'a SqurustConnection, sql: &str) -> Self {
        Query {
            conn,
            sql: sql.to_string(),
            params: Vec::new(),
        }
    }

    /// Append a positional bind parameter.
    pub fn bind(mut self, value: impl Into<Value>) -> Self {
        self.params.push(value.into());
        self
    }

    pub async fn fetch_all<T: FromRow>(self) -> Result<Vec<T>> {
        let engine = self.conn.engine().clone();
        let mut exec = engine.query(&self.sql, &self.params).await?;
        let names = column_names(&*exec);
        let mut out = Vec::new();
        while let Some(row) = exec.next().await? {
            let srow = SqurustRow::new(row, names.clone());
            out.push(T::from_row(&srow)?);
        }
        Ok(out)
    }

    pub async fn fetch_optional<T: FromRow>(self) -> Result<Option<T>> {
        let engine = self.conn.engine().clone();
        let mut exec = engine.query(&self.sql, &self.params).await?;
        let names = column_names(&*exec);
        match exec.next().await? {
            Some(row) => {
                let srow = SqurustRow::new(row, names);
                Ok(Some(T::from_row(&srow)?))
            }
            None => Ok(None),
        }
    }

    pub async fn fetch_one<T: FromRow>(self) -> Result<T> {
        self.fetch_optional()
            .await?
            .ok_or(SqurustError::RowNotFound)
    }

    /// Stream rows lazily.
    pub fn fetch_stream<T: FromRow + Send + 'static>(self) -> RowStream<T> {
        let engine = self.conn.engine().clone();
        let sql = self.sql;
        let params = self.params;
        let stream = async_stream::try_stream! {
            let mut exec = engine.query(&sql, &params).await?;
            let names = column_names(&*exec);
            while let Some(row) = exec.next().await? {
                let srow = SqurustRow::new(row, names.clone());
                yield T::from_row(&srow)?;
            }
        };
        RowStream::new(stream)
    }

    /// Execute as a statement, returning rows affected.
    pub async fn execute(self) -> Result<u64> {
        self.conn
            .engine()
            .execute(&self.sql, &self.params)
            .await
            .map_err(Into::into)
    }
}

pub(crate) fn column_names(exec: &dyn Executor) -> Arc<[String]> {
    exec.columns()
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .into()
}
