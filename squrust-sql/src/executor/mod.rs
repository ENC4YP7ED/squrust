//! Volcano-style execution operators. Each operator pulls rows from its input
//! via [`Executor::next`]. Read access comes from a shared `ReadTx`.

pub mod aggregate;
pub mod datetime;
pub mod distinct;
pub mod dml;
pub mod eval;
pub mod filter;
pub mod join;
pub mod limit;
pub mod projection;
pub mod scan;
pub mod sort;

use std::sync::Arc;

use async_trait::async_trait;
use squrust_core::PageSource;

use crate::error::Result;
use crate::planner::{ColumnInfo, LogicalPlan};
use crate::row::Row;
use crate::types::Value;

/// Shared, immutable bind parameters.
pub type Params = Arc<[Value]>;

/// A shared read source for scans: either a read or a write transaction.
pub type ReadSource = Arc<dyn PageSource + Send + Sync>;

#[async_trait]
pub trait Executor: Send {
    fn columns(&self) -> &[ColumnInfo];
    async fn next(&mut self) -> Result<Option<Row>>;

    /// Drain the executor into a vector of rows.
    async fn collect_all(&mut self) -> Result<Vec<Row>> {
        let mut out = Vec::new();
        while let Some(row) = self.next().await? {
            out.push(row);
        }
        Ok(out)
    }
}

/// An executor that yields a single empty row (for `SELECT <expr>` with no FROM).
pub struct DualExec {
    columns: Vec<ColumnInfo>,
    done: bool,
}

impl DualExec {
    fn new() -> Self {
        DualExec {
            columns: vec![],
            done: false,
        }
    }
}

#[async_trait]
impl Executor for DualExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    async fn next(&mut self) -> Result<Option<Row>> {
        if self.done {
            Ok(None)
        } else {
            self.done = true;
            Ok(Some(Row::new(0, vec![])))
        }
    }
}

/// An executor over a fixed, pre-computed set of rows (used by `PRAGMA`).
pub struct RowsExec {
    columns: Vec<ColumnInfo>,
    rows: std::vec::IntoIter<Row>,
}

impl RowsExec {
    pub fn new(columns: Vec<ColumnInfo>, rows: Vec<Vec<Value>>) -> Self {
        let rows: Vec<Row> = rows.into_iter().map(|v| Row::new(0, v)).collect();
        RowsExec {
            columns,
            rows: rows.into_iter(),
        }
    }
}

#[async_trait]
impl Executor for RowsExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    async fn next(&mut self) -> Result<Option<Row>> {
        Ok(self.rows.next())
    }
}

/// Build an executor tree from a logical plan.
pub fn build(plan: LogicalPlan, tx: ReadSource, params: Params) -> Box<dyn Executor> {
    match plan {
        LogicalPlan::Dual => Box::new(DualExec::new()),
        LogicalPlan::Scan {
            root_page,
            columns,
            rowid_alias,
            defaults,
            ..
        } => Box::new(scan::TableScan::new(
            tx, root_page, columns, rowid_alias, defaults,
        )),
        LogicalPlan::Filter { input, predicate } => {
            let inner = build(*input, tx, params.clone());
            Box::new(filter::FilterExec::new(inner, predicate, params))
        }
        LogicalPlan::Project {
            input,
            exprs,
            columns,
        } => {
            let inner = build(*input, tx, params.clone());
            Box::new(projection::ProjectExec::new(inner, exprs, columns, params))
        }
        LogicalPlan::NestedLoopJoin {
            left,
            right,
            predicate,
            left_outer,
            columns,
        } => {
            let l = build(*left, tx.clone(), params.clone());
            let r = build(*right, tx, params.clone());
            Box::new(join::NestedLoopJoin::new(
                l, r, predicate, left_outer, columns, params,
            ))
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggs,
            output,
            columns,
            base_len,
            having,
        } => {
            let inner = build(*input, tx, params.clone());
            Box::new(aggregate::AggExec::new(
                inner, group_by, aggs, output, columns, base_len, having, params,
            ))
        }
        LogicalPlan::Sort { input, keys } => {
            let inner = build(*input, tx, params.clone());
            Box::new(sort::SortExec::new(inner, keys, params))
        }
        LogicalPlan::Limit {
            input,
            limit,
            offset,
        } => {
            let inner = build(*input, tx, params);
            Box::new(limit::LimitExec::new(inner, limit, offset))
        }
        LogicalPlan::Distinct { input } => {
            let inner = build(*input, tx, params);
            Box::new(distinct::DistinctExec::new(inner))
        }
    }
}
