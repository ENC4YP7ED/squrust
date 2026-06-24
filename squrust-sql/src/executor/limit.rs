//! LIMIT / OFFSET.

use async_trait::async_trait;

use crate::error::Result;
use crate::executor::Executor;
use crate::planner::ColumnInfo;
use crate::row::Row;

pub struct LimitExec {
    input: Box<dyn Executor>,
    limit: Option<u64>,
    offset: u64,
    columns: Vec<ColumnInfo>,
    skipped: u64,
    emitted: u64,
}

impl LimitExec {
    pub fn new(input: Box<dyn Executor>, limit: Option<u64>, offset: u64) -> Self {
        let columns = input.columns().to_vec();
        LimitExec {
            input,
            limit,
            offset,
            columns,
            skipped: 0,
            emitted: 0,
        }
    }
}

#[async_trait]
impl Executor for LimitExec {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        while self.skipped < self.offset {
            if self.input.next().await?.is_none() {
                return Ok(None);
            }
            self.skipped += 1;
        }
        if let Some(l) = self.limit {
            if self.emitted >= l {
                return Ok(None);
            }
        }
        match self.input.next().await? {
            Some(row) => {
                self.emitted += 1;
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }
}
