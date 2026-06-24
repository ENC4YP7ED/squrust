//! In-memory schema model: tables, columns and indexes.

pub mod catalog;

use squrust_core::PageId;

use crate::types::SqlType;
use crate::types::Value;

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub sql_type: SqlType,
    pub not_null: bool,
    pub primary_key: bool,
    pub default: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// Root page of this table's data b-tree.
    pub root_page: PageId,
    /// Index of an `INTEGER PRIMARY KEY` column that aliases the row id.
    pub rowid_alias: Option<usize>,
    /// The original `CREATE TABLE` text (stored in the catalog).
    pub sql: String,
}

impl Table {
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.column_index(name).map(|i| &self.columns[i])
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub sql: String,
}
