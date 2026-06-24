//! The schema catalog, persisted in a `sqlite_master`-style table on a fixed
//! root page and mirrored in memory for fast planning.

use std::collections::HashMap;

use squrust_core::{BTree, PageId, PageSink, PageSource};

use crate::ddl;
use crate::error::{Result, SqlError};
use crate::row::{Row, RowId};
use crate::schema::{Index, Table};
use crate::types::Value;

/// Root page of the catalog b-tree. This is `sqlite_master`, which SQLite
/// always roots at page 1 (its b-tree header sits just past the 100-byte file
/// header).
pub const CATALOG_ROOT: PageId = 1;

/// In-memory schema. Keys are lower-cased names.
#[derive(Debug, Default)]
pub struct Catalog {
    pub tables: HashMap<String, Table>,
    pub indexes: HashMap<String, Index>,
    /// name (lower) -> row id of the catalog entry, for DROP.
    catalog_ids: HashMap<String, RowId>,
}

impl Catalog {
    pub fn get_table(&self, name: &str) -> Option<&Table> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    pub fn table_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tables.values().map(|t| t.name.clone()).collect();
        v.sort();
        v
    }

    /// Rebuild the in-memory catalog by scanning the catalog b-tree.
    pub fn load<S: PageSource>(src: &S) -> Result<Catalog> {
        let mut catalog = Catalog::default();
        let tree = BTree::open(CATALOG_ROOT);
        let mut cursor = tree.cursor(src)?;
        while let Some((rowid, bytes)) = cursor.next()? {
            let row = Row::decode(rowid, &bytes)?;
            catalog.absorb(rowid, &row)?;
        }
        Ok(catalog)
    }

    fn absorb(&mut self, rowid: RowId, row: &Row) -> Result<()> {
        let entry_type = text(&row.values, 0);
        let name = text(&row.values, 1);
        let rootpage = row.values.get(3).and_then(|v| v.as_i64()).unwrap_or(0) as PageId;
        let sql = text(&row.values, 4);

        match entry_type.as_str() {
            "table" => {
                let mut table = ddl::parse_create_table(&sql)?;
                table.root_page = rootpage;
                self.catalog_ids.insert(name.to_ascii_lowercase(), rowid);
                self.tables.insert(table.name.to_ascii_lowercase(), table);
            }
            "index" => {
                let index = ddl::parse_create_index(&sql)?;
                self.catalog_ids.insert(name.to_ascii_lowercase(), rowid);
                self.indexes.insert(index.name.to_ascii_lowercase(), index);
            }
            other => {
                return Err(SqlError::Schema(format!("unknown catalog entry {other}")));
            }
        }
        Ok(())
    }

    /// Persist a new table and add it to the in-memory catalog.
    pub fn add_table<S: PageSink>(&mut self, sink: &S, mut table: Table) -> Result<()> {
        let rowid = insert_entry(
            sink,
            "table",
            &table.name,
            &table.name,
            table.root_page,
            &table.sql,
        )?;
        self.catalog_ids.insert(table.name.to_ascii_lowercase(), rowid);
        table.name.shrink_to_fit();
        self.tables.insert(table.name.to_ascii_lowercase(), table);
        Ok(())
    }

    pub fn add_index<S: PageSink>(&mut self, _sink: &S, index: Index) -> Result<()> {
        // Indexes are tracked in memory only: Squrust does not build index
        // b-trees yet (the executor table-scans), and writing an index row to
        // `sqlite_master` with rootpage 0 would make the file invalid for
        // stock sqlite. So we keep the on-disk file clean and index-free.
        self.indexes.insert(index.name.to_ascii_lowercase(), index);
        Ok(())
    }

    /// Append a column to a table: rewrite its `sqlite_master.sql` in place
    /// (same rowid and rootpage) and update the in-memory schema. Existing row
    /// data is left untouched — short records are padded on read.
    pub fn alter_add_column<S: PageSink>(
        &mut self,
        sink: &S,
        table_name: &str,
        column: crate::schema::Column,
        new_sql: String,
    ) -> Result<()> {
        let key = table_name.to_ascii_lowercase();
        let table = self
            .tables
            .get_mut(&key)
            .ok_or_else(|| SqlError::NotFound(format!("table `{table_name}`")))?;
        let rowid = *self
            .catalog_ids
            .get(&key)
            .ok_or_else(|| SqlError::NotFound(format!("table `{table_name}`")))?;

        let row = Row::new(
            rowid,
            vec![
                Value::Text("table".to_string()),
                Value::Text(table.name.clone()),
                Value::Text(table.name.clone()),
                Value::Integer(table.root_page as i64),
                Value::Text(new_sql.clone()),
            ],
        );
        let tree = BTree::open(CATALOG_ROOT);
        tree.insert(sink, rowid, &row.encode())?;

        table.columns.push(column);
        table.sql = new_sql;
        Ok(())
    }

    /// Remove a table from the catalog (both persisted and in-memory).
    pub fn drop_table<S: PageSink>(&mut self, sink: &S, name: &str) -> Result<bool> {
        let key = name.to_ascii_lowercase();
        if !self.tables.contains_key(&key) {
            return Ok(false);
        }
        if let Some(rowid) = self.catalog_ids.remove(&key) {
            let tree = BTree::open(CATALOG_ROOT);
            tree.delete(sink, rowid)?;
        }
        self.tables.remove(&key);
        // Drop dependent indexes.
        let dependent: Vec<String> = self
            .indexes
            .values()
            .filter(|i| i.table.eq_ignore_ascii_case(name))
            .map(|i| i.name.to_ascii_lowercase())
            .collect();
        for idx_key in dependent {
            if let Some(rowid) = self.catalog_ids.remove(&idx_key) {
                let tree = BTree::open(CATALOG_ROOT);
                tree.delete(sink, rowid)?;
            }
            self.indexes.remove(&idx_key);
        }
        Ok(true)
    }
}

fn text(values: &[Value], idx: usize) -> String {
    match values.get(idx) {
        Some(v) => v.to_display_string(),
        None => String::new(),
    }
}

fn insert_entry<S: PageSink>(
    sink: &S,
    entry_type: &str,
    name: &str,
    tbl_name: &str,
    rootpage: PageId,
    sql: &str,
) -> Result<RowId> {
    let tree = BTree::open(CATALOG_ROOT);
    let next = tree.last_key(sink)?.unwrap_or(0) + 1;
    let row = Row::new(
        next,
        vec![
            Value::Text(entry_type.to_string()),
            Value::Text(name.to_string()),
            Value::Text(tbl_name.to_string()),
            Value::Integer(rootpage as i64),
            Value::Text(sql.to_string()),
        ],
    );
    tree.insert(sink, next, &row.encode())?;
    Ok(next)
}
