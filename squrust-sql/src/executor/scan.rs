//! Full table scan over a table's b-tree.

use async_trait::async_trait;
use squrust_core::{BTree, PageId};

use crate::error::Result;
use crate::executor::{Executor, ReadSource};
use crate::planner::ColumnInfo;
use crate::row::Row;
use crate::types::Value;

pub struct TableScan {
    tx: ReadSource,
    root: PageId,
    columns: Vec<ColumnInfo>,
    /// Index of the column that aliases the rowid (`INTEGER PRIMARY KEY`).
    rowid_alias: Option<usize>,
    /// Per-column constant defaults; pads records written before an
    /// `ALTER TABLE ADD COLUMN` widened the schema.
    defaults: Vec<Value>,
    next_key: i64,
    done: bool,
}

impl TableScan {
    pub fn new(
        tx: ReadSource,
        root: PageId,
        columns: Vec<ColumnInfo>,
        rowid_alias: Option<usize>,
        defaults: Vec<Value>,
    ) -> Self {
        TableScan {
            tx,
            root,
            columns,
            rowid_alias,
            defaults,
            next_key: i64::MIN,
            done: false,
        }
    }
}

#[async_trait]
impl Executor for TableScan {
    fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    async fn next(&mut self) -> Result<Option<Row>> {
        if self.done {
            return Ok(None);
        }
        let tree = BTree::open(self.root);
        // Re-seek each call; the cursor borrows the tx only for this call.
        let mut cursor = tree.cursor_from(&*self.tx, self.next_key)?;
        match cursor.next()? {
            Some((key, bytes)) => {
                let mut row = Row::decode(key, &bytes)?;
                // Records written before an ADD COLUMN are short; pad the
                // trailing columns with their constant defaults.
                if row.values.len() < self.columns.len() {
                    row.values
                        .extend_from_slice(&self.defaults[row.values.len()..]);
                }
                // The rowid-alias column is stored as NULL; its value is the rowid.
                if let Some(a) = self.rowid_alias {
                    if a < row.values.len() {
                        row.values[a] = Value::Integer(key);
                    }
                }
                match key.checked_add(1) {
                    Some(k) => self.next_key = k,
                    None => self.done = true,
                }
                Ok(Some(row))
            }
            None => {
                self.done = true;
                Ok(None)
            }
        }
    }
}
