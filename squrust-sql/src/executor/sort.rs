//! In-memory ORDER BY.

use std::cmp::Ordering;

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::eval::eval;
use crate::executor::{Executor, Params};
use crate::planner::{ColumnInfo, SortKey};
use crate::row::Row;
use crate::types::Value;

pub struct SortExec {
    input: Option<Box<dyn Executor>>,
    keys: Vec<SortKey>,
    columns: Vec<ColumnInfo>,
    params: Params,
    produced: Option<std::vec::IntoIter<Row>>,
}

impl SortExec {
    pub fn new(input: Box<dyn Executor>, keys: Vec<SortKey>, params: Params) -> Self {
        let columns = input.columns().to_vec();
        SortExec {
            input: Some(input),
            keys,
            columns,
            params,
            produced: None,
        }
    }

    async fn sort_rows(&mut self) -> Result<Vec<Row>> {
        let mut input = self.input.take().expect("sort computed once");
        let rows = input.collect_all().await?;

        // Precompute sort keys so the comparator is infallible.
        let mut keyed: Vec<(Vec<Value>, Row)> = Vec::with_capacity(rows.len());
        for row in rows {
            let mut key = Vec::with_capacity(self.keys.len());
            for k in &self.keys {
                key.push(eval(&k.expr, &row.values, row.row_id, &self.params)?);
            }
            keyed.push((key, row));
        }

        keyed.sort_by(|a, b| {
            for (i, k) in self.keys.iter().enumerate() {
                let ord = a.0[i].order_key(&b.0[i]);
                let ord = if k.asc { ord } else { ord.reverse() };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });

        Ok(keyed.into_iter().map(|(_, r)| r).collect())
    }
}

#[async_trait]
impl Executor for SortExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        if self.produced.is_none() {
            let rows = self.sort_rows().await?;
            self.produced = Some(rows.into_iter());
        }
        Ok(self.produced.as_mut().unwrap().next())
    }
}
