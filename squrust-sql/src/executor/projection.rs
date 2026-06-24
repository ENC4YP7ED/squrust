//! Projection: evaluate the select-list expressions for each input row.

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::eval::eval;
use crate::executor::{Executor, Params};
use crate::planner::{ColumnInfo, Expr};
use crate::row::Row;

pub struct ProjectExec {
    input: Box<dyn Executor>,
    exprs: Vec<Expr>,
    columns: Vec<ColumnInfo>,
    params: Params,
}

impl ProjectExec {
    pub fn new(
        input: Box<dyn Executor>,
        exprs: Vec<Expr>,
        columns: Vec<ColumnInfo>,
        params: Params,
    ) -> Self {
        ProjectExec {
            input,
            exprs,
            columns,
            params,
        }
    }
}

#[async_trait]
impl Executor for ProjectExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        match self.input.next().await? {
            Some(row) => {
                let mut values = Vec::with_capacity(self.exprs.len());
                for e in &self.exprs {
                    values.push(eval(e, &row.values, row.row_id, &self.params)?);
                }
                Ok(Some(Row::new(row.row_id, values)))
            }
            None => Ok(None),
        }
    }
}
