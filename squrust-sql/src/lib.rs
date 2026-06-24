//! # squrust-sql
//!
//! SQL parser, planner and execution engine layered on top of
//! `squrust-core` transactions.

#![forbid(unsafe_code)]

pub mod ddl;
pub mod error;
pub mod executor;
pub mod parser;
pub mod planner;
pub mod row;
pub mod schema;
pub mod types;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use squrust_core::{BTree, StorageEngine, WriteTx};

pub use error::{Result, SqlError};
pub use executor::{Executor, ReadSource};
pub use planner::ColumnInfo;
pub use row::{Row, RowId};
pub use types::{SqlType, Value};

use executor::dml;
use planner::{Plan, plan};
use schema::Table;
use schema::catalog::{CATALOG_ROOT, Catalog};

/// The SQL engine: owns the storage engine and the schema catalog.
pub struct SqlEngine {
    storage: Arc<StorageEngine>,
    catalog: Mutex<Catalog>,
    last_insert_rowid: AtomicI64,
    changes: AtomicI64,
}

impl SqlEngine {
    /// Open the SQL engine over a storage engine, creating the catalog if the
    /// database is brand new and loading the schema otherwise.
    pub async fn new(storage: Arc<StorageEngine>) -> Result<Arc<SqlEngine>> {
        // Page 1 is `sqlite_master`; a freshly created file already holds an
        // empty leaf there, so there is nothing to create — just load.
        let _ = CATALOG_ROOT;
        let catalog = {
            let rtx = storage.begin_read();
            Catalog::load(&rtx)?
        };
        Ok(Arc::new(SqlEngine {
            storage,
            catalog: Mutex::new(catalog),
            last_insert_rowid: AtomicI64::new(0),
            changes: AtomicI64::new(0),
        }))
    }

    pub fn storage(&self) -> &Arc<StorageEngine> {
        &self.storage
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.last_insert_rowid.load(Ordering::SeqCst)
    }

    pub fn changes(&self) -> i64 {
        self.changes.load(Ordering::SeqCst)
    }

    /// List user table names (for `.tables` and similar).
    pub fn table_names(&self) -> Vec<String> {
        self.catalog.lock().unwrap().table_names()
    }

    /// The stored `CREATE TABLE` text for every table, ordered by name.
    pub fn schema_statements(&self) -> Vec<String> {
        let catalog = self.catalog.lock().unwrap();
        let mut pairs: Vec<(String, String)> = catalog
            .tables
            .values()
            .map(|t| (t.name.clone(), t.sql.clone()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs.into_iter().map(|(_, sql)| sql).collect()
    }

    /// The stored `CREATE TABLE` text for a single table.
    pub fn table_sql(&self, name: &str) -> Option<String> {
        self.catalog.lock().unwrap().get_table(name).map(|t| t.sql.clone())
    }

    /// The column metadata a query would produce, without executing it.
    pub fn describe(&self, sql: &str) -> Result<Vec<ColumnInfo>> {
        let stmt = single_statement(sql)?;
        match self.plan_stmt(&stmt)? {
            Plan::Query { columns, .. } => Ok(columns),
            _ => Ok(vec![]),
        }
    }

    fn plan_stmt(&self, stmt: &sqlparser::ast::Statement) -> Result<Plan> {
        let catalog = self.catalog.lock().unwrap();
        plan(stmt, &catalog)
    }

    fn clone_table(&self, name: &str) -> Result<Table> {
        let catalog = self.catalog.lock().unwrap();
        catalog
            .get_table(name)
            .cloned()
            .ok_or_else(|| SqlError::NotFound(format!("table `{name}`")))
    }

    /// Run DDL statements (CREATE/DROP). Equivalent to `execute` discarding the
    /// count.
    pub async fn execute_ddl(&self, sql: &str) -> Result<()> {
        self.execute(sql, &[]).await.map(|_| ())
    }

    /// Run a query and return a streaming row executor. Requires a single
    /// SELECT statement. Reads from a fresh snapshot.
    pub async fn query(&self, sql: &str, params: &[Value]) -> Result<Box<dyn Executor>> {
        let source: ReadSource = Arc::new(self.storage.begin_read());
        self.build_query(source, sql, params)
    }

    /// Plan and build a query executor reading from an arbitrary source (a read
    /// snapshot or an in-flight write transaction).
    pub fn build_query(
        &self,
        source: ReadSource,
        sql: &str,
        params: &[Value],
    ) -> Result<Box<dyn Executor>> {
        let stmt = single_statement(sql)?;
        match self.plan_stmt(&stmt)? {
            Plan::Query { plan, .. } => {
                let params: executor::Params = params.to_vec().into();
                Ok(executor::build(plan, source, params))
            }
            _ => Err(SqlError::Unsupported(
                "query() requires a SELECT statement; use execute() for DML/DDL".into(),
            )),
        }
    }

    /// Execute DML (INSERT/UPDATE/DELETE) against an in-flight write transaction
    /// without committing it. DDL and SELECT are rejected here.
    pub async fn execute_on(&self, tx: &WriteTx, sql: &str, params: &[Value]) -> Result<u64> {
        let statements = parser::parse(sql)?;
        let mut affected = 0u64;
        for stmt in &statements {
            affected = match self.plan_stmt(stmt)? {
                Plan::Insert(p) => {
                    let table = self.clone_table(&p.table)?;
                    let res = dml::insert(tx, &table, &p, params)?;
                    self.last_insert_rowid
                        .store(res.last_rowid, Ordering::SeqCst);
                    self.changes.store(res.count as i64, Ordering::SeqCst);
                    res.count
                }
                Plan::Update(p) => {
                    let table = self.clone_table(&p.table)?;
                    let count = dml::update(tx, &table, &p, params)?;
                    self.changes.store(count as i64, Ordering::SeqCst);
                    count
                }
                Plan::Delete(p) => {
                    let table = self.clone_table(&p.table)?;
                    let count = dml::delete(tx, &table, &p, params)?;
                    self.changes.store(count as i64, Ordering::SeqCst);
                    count
                }
                _ => {
                    return Err(SqlError::Unsupported(
                        "only INSERT/UPDATE/DELETE are allowed inside a transaction".into(),
                    ));
                }
            };
        }
        Ok(affected)
    }

    /// Execute one or more statements, returning the rows affected by the last
    /// data-modification statement (0 for DDL / SELECT).
    pub async fn execute(&self, sql: &str, params: &[Value]) -> Result<u64> {
        let statements = parser::parse(sql)?;
        let mut affected = 0u64;
        for stmt in &statements {
            affected = self.run_one(stmt, params).await?;
        }
        Ok(affected)
    }

    async fn run_one(&self, stmt: &sqlparser::ast::Statement, params: &[Value]) -> Result<u64> {
        match self.plan_stmt(stmt)? {
            Plan::Query { plan, .. } => {
                // Run and discard rows.
                let source: ReadSource = Arc::new(self.storage.begin_read());
                let params: executor::Params = params.to_vec().into();
                let mut exec = executor::build(plan, source, params);
                while exec.next().await?.is_some() {}
                Ok(0)
            }
            Plan::Insert(p) => {
                let table = self.clone_table(&p.table)?;
                let tx = self.storage.begin_write();
                let res = dml::insert(&tx, &table, &p, params)?;
                tx.commit()?;
                self.last_insert_rowid
                    .store(res.last_rowid, Ordering::SeqCst);
                self.changes.store(res.count as i64, Ordering::SeqCst);
                Ok(res.count)
            }
            Plan::Update(p) => {
                let table = self.clone_table(&p.table)?;
                let tx = self.storage.begin_write();
                let count = dml::update(&tx, &table, &p, params)?;
                tx.commit()?;
                self.changes.store(count as i64, Ordering::SeqCst);
                Ok(count)
            }
            Plan::Delete(p) => {
                let table = self.clone_table(&p.table)?;
                let tx = self.storage.begin_write();
                let count = dml::delete(&tx, &table, &p, params)?;
                tx.commit()?;
                self.changes.store(count as i64, Ordering::SeqCst);
                Ok(count)
            }
            Plan::CreateTable {
                mut table,
                if_not_exists,
            } => {
                let mut catalog = self.catalog.lock().unwrap();
                if catalog.get_table(&table.name).is_some() {
                    if if_not_exists {
                        return Ok(0);
                    }
                    return Err(SqlError::Constraint(format!(
                        "table {} already exists",
                        table.name
                    )));
                }
                let tx = self.storage.begin_write();
                table.root_page = BTree::create(&tx)?;
                catalog.add_table(&tx, table)?;
                tx.commit()?;
                Ok(0)
            }
            Plan::CreateIndex {
                index,
                if_not_exists,
            } => {
                let mut catalog = self.catalog.lock().unwrap();
                if catalog.indexes.contains_key(&index.name.to_ascii_lowercase()) {
                    if if_not_exists {
                        return Ok(0);
                    }
                    return Err(SqlError::Constraint(format!(
                        "index {} already exists",
                        index.name
                    )));
                }
                let tx = self.storage.begin_write();
                catalog.add_index(&tx, index)?;
                tx.commit()?;
                Ok(0)
            }
            Plan::DropTable { name, if_exists } => {
                let mut catalog = self.catalog.lock().unwrap();
                let tx = self.storage.begin_write();
                let dropped = catalog.drop_table(&tx, &name)?;
                tx.commit()?;
                if !dropped && !if_exists {
                    return Err(SqlError::NotFound(format!("table `{name}`")));
                }
                Ok(0)
            }
            Plan::AlterTableAddColumn {
                table,
                column,
                new_sql,
            } => {
                let mut catalog = self.catalog.lock().unwrap();
                let tx = self.storage.begin_write();
                catalog.alter_add_column(&tx, &table, column, new_sql)?;
                tx.commit()?;
                Ok(0)
            }
        }
    }
}

fn single_statement(sql: &str) -> Result<sqlparser::ast::Statement> {
    let mut statements = parser::parse(sql)?;
    if statements.len() != 1 {
        return Err(SqlError::Parse(format!(
            "expected a single statement, found {}",
            statements.len()
        )));
    }
    Ok(statements.pop().unwrap())
}
