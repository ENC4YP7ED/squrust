//! Filter: keep rows for which the predicate is truthy.

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::eval::eval;
use crate::executor::{Executor, Params};
use crate::planner::{ColumnInfo, Expr};
use crate::row::Row;

pub struct FilterExec {
    input: Box<dyn Executor>,
    predicate: Expr,
    columns: Vec<ColumnInfo>,
    params: Params,
}

impl FilterExec {
    pub fn new(input: Box<dyn Executor>, predicate: Expr, params: Params) -> Self {
        let columns = input.columns().to_vec();
        FilterExec {
            input,
            predicate,
            columns,
            params,
        }
    }
}

#[async_trait]
impl Executor for FilterExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        while let Some(row) = self.input.next().await? {
            let v = eval(&self.predicate, &row.values, row.row_id, &self.params)?;
            if v.is_truthy() {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
}
