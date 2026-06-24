//! A typed result row.

use std::sync::Arc;

use squrust_serde::{FromValue, RowAccess};
use squrust_sql::{Row, Value};

use crate::error::{Result, SqurustError};

/// One row of a query result, with column names for by-name access.
#[derive(Debug, Clone)]
pub struct SqurustRow {
    values: Vec<Value>,
    names: Arc<[String]>,
    row_id: i64,
}

impl SqurustRow {
    pub fn new(row: Row, names: Arc<[String]>) -> Self {
        SqurustRow {
            row_id: row.row_id,
            values: row.values,
            names,
        }
    }

    /// Typed column access by position.
    pub fn get<T: FromValue>(&self, idx: usize) -> Result<T> {
        RowAccess::get(self, idx).map_err(SqurustError::from)
    }

    /// Typed column access by name.
    pub fn get_by_name<T: FromValue>(&self, name: &str) -> Result<T> {
        RowAccess::get_by_name(self, name).map_err(SqurustError::from)
    }

    pub fn column_count(&self) -> usize {
        self.values.len()
    }

    pub fn column_name(&self, idx: usize) -> Option<&str> {
        self.names.get(idx).map(|s| s.as_str())
    }

    /// The row's id (b-tree key).
    pub fn row_id(&self) -> i64 {
        self.row_id
    }
}

impl RowAccess for SqurustRow {
    fn value(&self, idx: usize) -> Option<&Value> {
        self.values.get(idx)
    }
    fn value_by_name(&self, name: &str) -> Option<&Value> {
        self.names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .and_then(|i| self.values.get(i))
    }
    fn ncols(&self) -> usize {
        self.values.len()
    }
}
