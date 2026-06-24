//! Set operations: `UNION [ALL]`, `INTERSECT`, `EXCEPT`.
//!
//! Both inputs are materialized, then combined. `UNION ALL` concatenates and
//! preserves order; the distinct variants deduplicate and — matching SQLite,
//! which dedups through a sorter — return rows ordered by all columns.

use std::cmp::Ordering;

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::distinct::rows_eq;
use crate::executor::Executor;
use crate::planner::{ColumnInfo, SetOpKind};
use crate::row::Row;
use crate::types::Value;

pub struct SetOpExec {
    left: Option<Box<dyn Executor>>,
    right: Option<Box<dyn Executor>>,
    kind: SetOpKind,
    all: bool,
    columns: Vec<ColumnInfo>,
    produced: Option<std::vec::IntoIter<Row>>,
}

impl SetOpExec {
    pub fn new(
        left: Box<dyn Executor>,
        right: Box<dyn Executor>,
        kind: SetOpKind,
        all: bool,
        columns: Vec<ColumnInfo>,
    ) -> Self {
        SetOpExec {
            left: Some(left),
            right: Some(right),
            kind,
            all,
            columns,
            produced: None,
        }
    }

    async fn compute(&mut self) -> Result<Vec<Row>> {
        let lrows = self.left.take().expect("computed once").collect_all().await?;
        let rrows = self.right.take().expect("computed once").collect_all().await?;

        match self.kind {
            SetOpKind::Union if self.all => Ok(lrows.into_iter().chain(rrows).collect()),
            SetOpKind::Union => {
                let mut out = dedup(lrows.into_iter().chain(rrows));
                sort_rows(&mut out);
                Ok(out)
            }
            SetOpKind::Intersect => {
                let keep = |row: &Row| rrows.iter().any(|r| rows_eq(&r.values, &row.values));
                let mut out = dedup(lrows.into_iter().filter(keep));
                sort_rows(&mut out);
                Ok(out)
            }
            SetOpKind::Except => {
                let drop = |row: &Row| rrows.iter().any(|r| rows_eq(&r.values, &row.values));
                let mut out = dedup(lrows.into_iter().filter(|r| !drop(r)));
                sort_rows(&mut out);
                Ok(out)
            }
        }
    }
}

#[async_trait]
impl Executor for SetOpExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        if self.produced.is_none() {
            let rows = self.compute().await?;
            self.produced = Some(rows.into_iter());
        }
        Ok(self.produced.as_mut().unwrap().next())
    }
}

/// Keep the first occurrence of each distinct row (SQLite set equality).
fn dedup(rows: impl Iterator<Item = Row>) -> Vec<Row> {
    let mut seen: Vec<Vec<Value>> = Vec::new();
    let mut out = Vec::new();
    for row in rows {
        if !seen.iter().any(|s| rows_eq(s, &row.values)) {
            seen.push(row.values.clone());
            out.push(row);
        }
    }
    out
}

fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| cmp_values(&a.values, &b.values));
}

fn cmp_values(a: &[Value], b: &[Value]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = x.order_key(y);
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}
