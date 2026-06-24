//! `SELECT DISTINCT`: drop duplicate output rows, preserving first-seen order.

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::Executor;
use crate::planner::ColumnInfo;
use crate::row::Row;
use crate::types::Value;

pub struct DistinctExec {
    input: Box<dyn Executor>,
    columns: Vec<ColumnInfo>,
    seen: Vec<Vec<Value>>,
}

impl DistinctExec {
    pub fn new(input: Box<dyn Executor>) -> Self {
        let columns = input.columns().to_vec();
        DistinctExec {
            input,
            columns,
            seen: Vec::new(),
        }
    }
}

#[async_trait]
impl Executor for DistinctExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        while let Some(row) = self.input.next().await? {
            // Two rows are duplicates iff their value lists compare equal under
            // SQLite value semantics (NULLs are considered equal for DISTINCT).
            let dup = self.seen.iter().any(|prev| rows_eq(prev, &row.values));
            if !dup {
                self.seen.push(row.values.clone());
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
}

fn rows_eq(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x.is_null(), y.is_null()) {
            (true, true) => true,
            (false, false) => x == y,
            _ => false,
        })
}
