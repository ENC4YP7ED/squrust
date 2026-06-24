//! Nested-loop join (inner and left-outer). The right side is materialised once
//! and re-scanned for every left row.

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::eval::eval;
use crate::executor::{Executor, Params};
use crate::planner::{ColumnInfo, Expr};
use crate::row::Row;
use crate::types::Value;

pub struct NestedLoopJoin {
    left: Box<dyn Executor>,
    right: Option<Box<dyn Executor>>,
    right_rows: Vec<Row>,
    right_ready: bool,
    predicate: Option<Expr>,
    left_outer: bool,
    columns: Vec<ColumnInfo>,
    params: Params,
    right_ncols: usize,
    // iteration state
    cur_left: Option<Row>,
    right_idx: usize,
    matched: bool,
}

impl NestedLoopJoin {
    pub fn new(
        left: Box<dyn Executor>,
        right: Box<dyn Executor>,
        predicate: Option<Expr>,
        left_outer: bool,
        columns: Vec<ColumnInfo>,
        params: Params,
    ) -> Self {
        let right_ncols = right.columns().len();
        NestedLoopJoin {
            left,
            right: Some(right),
            right_rows: Vec::new(),
            right_ready: false,
            predicate,
            left_outer,
            columns,
            params,
            right_ncols,
            cur_left: None,
            right_idx: 0,
            matched: false,
        }
    }

    async fn ensure_right(&mut self) -> Result<()> {
        if !self.right_ready {
            if let Some(mut r) = self.right.take() {
                self.right_rows = r.collect_all().await?;
            }
            self.right_ready = true;
        }
        Ok(())
    }
}

#[async_trait]
impl Executor for NestedLoopJoin {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        self.ensure_right().await?;

        loop {
            if self.cur_left.is_none() {
                match self.left.next().await? {
                    Some(row) => {
                        self.cur_left = Some(row);
                        self.right_idx = 0;
                        self.matched = false;
                    }
                    None => return Ok(None),
                }
            }
            let left_row = self.cur_left.clone().unwrap();

            while self.right_idx < self.right_rows.len() {
                let right_row = &self.right_rows[self.right_idx];
                self.right_idx += 1;
                let mut combined = left_row.values.clone();
                combined.extend_from_slice(&right_row.values);
                let keep = match &self.predicate {
                    None => true,
                    Some(p) => eval(p, &combined, left_row.row_id, &self.params)?.is_truthy(),
                };
                if keep {
                    self.matched = true;
                    return Ok(Some(Row::new(left_row.row_id, combined)));
                }
            }

            // Right side exhausted for this left row.
            let emit_outer = self.left_outer && !self.matched;
            self.cur_left = None;
            if emit_outer {
                let mut combined = left_row.values.clone();
                combined.extend(std::iter::repeat(Value::Null).take(self.right_ncols));
                return Ok(Some(Row::new(left_row.row_id, combined)));
            }
            // otherwise loop to fetch the next left row
        }
    }
}
