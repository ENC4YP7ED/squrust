//! Data-modification operations (INSERT / UPDATE / DELETE) executed directly
//! against a write transaction.

use squrust_core::{BTree, WriteTx};

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, SqlError};
use crate::executor::eval::eval;
use crate::planner::{DeletePlan, InsertPlan, UpdatePlan};
use crate::row::Row;
use crate::schema::{Column, DefaultExpr, Table};
use crate::types::Value;

/// Result of an INSERT: rows affected and the last row id assigned.
pub struct InsertResult {
    pub count: u64,
    pub last_rowid: i64,
}

pub fn insert(
    tx: &WriteTx,
    table: &Table,
    plan: &InsertPlan,
    params: &[Value],
) -> Result<InsertResult> {
    let tree = BTree::open(table.root_page);
    let mut count = 0u64;
    let mut last_rowid = 0i64;

    for row_exprs in &plan.rows {
        if row_exprs.len() != plan.target_cols.len() {
            return Err(SqlError::Type(format!(
                "{} values for {} columns",
                row_exprs.len(),
                plan.target_cols.len()
            )));
        }

        // Start from defaults (dynamic keyword defaults evaluated now).
        let mut values: Vec<Value> = table.columns.iter().map(eval_default).collect();

        for (slot, expr) in plan.target_cols.iter().zip(row_exprs) {
            let v = eval(expr, &[], 0, params)?;
            values[*slot] = v.coerce_to(table.columns[*slot].sql_type)?;
        }

        // Determine the row id.
        let rowid = match table.rowid_alias {
            Some(idx) if !values[idx].is_null() => values[idx]
                .as_i64()
                .ok_or_else(|| SqlError::Type("INTEGER PRIMARY KEY must be an integer".into()))?,
            _ => tree.last_key(tx)?.unwrap_or(0) + 1,
        };
        if let Some(idx) = table.rowid_alias {
            values[idx] = Value::Integer(rowid);
        }

        // Enforce NOT NULL.
        for (i, col) in table.columns.iter().enumerate() {
            if col.not_null && values[i].is_null() {
                return Err(SqlError::Constraint(format!(
                    "NOT NULL constraint failed: {}.{}",
                    table.name, col.name
                )));
            }
        }

        // UNIQUE column constraints (non-rowid). Table-scanned in the absence of
        // index b-trees.
        if let Some((ci, conflict_rowid)) = find_unique_conflict(tx, &tree, table, &values, None)? {
            if plan.or_replace {
                tree.delete(tx, conflict_rowid)?;
            } else {
                return Err(SqlError::Constraint(format!(
                    "UNIQUE constraint failed: {}.{}",
                    table.name, table.columns[ci].name
                )));
            }
        }

        // Primary-key uniqueness for the rowid alias.
        let exists = tree.get(tx, rowid)?.is_some();
        if exists {
            if plan.or_replace {
                tree.delete(tx, rowid)?;
            } else if table.rowid_alias.is_some() {
                return Err(SqlError::Constraint(format!(
                    "UNIQUE constraint failed: {}.{}",
                    table.name,
                    table.rowid_alias.map(|i| table.columns[i].name.as_str()).unwrap_or("rowid")
                )));
            }
        }

        // Store the rowid-alias column as NULL (SQLite convention); its value
        // is the rowid, recovered on read.
        let row = Row::new(rowid, with_alias_nulled(values, table.rowid_alias));
        tree.insert(tx, rowid, &row.encode())?;
        count += 1;
        last_rowid = rowid;
    }

    Ok(InsertResult { count, last_rowid })
}

pub fn update(tx: &WriteTx, table: &Table, plan: &UpdatePlan, params: &[Value]) -> Result<u64> {
    let tree = BTree::open(table.root_page);
    let matching = scan_matching(tx, &tree, plan.predicate.as_ref(), params, table.rowid_alias)?;

    let mut count = 0u64;
    for (rowid, mut values) in matching {
        let mut new_rowid = rowid;
        for (col_idx, expr) in &plan.assignments {
            let v = eval(expr, &values, rowid, params)?;
            let coerced = v.coerce_to(table.columns[*col_idx].sql_type)?;
            if table.rowid_alias == Some(*col_idx) {
                new_rowid = coerced
                    .as_i64()
                    .ok_or_else(|| SqlError::Type("INTEGER PRIMARY KEY must be integer".into()))?;
            }
            values[*col_idx] = coerced;
        }

        // NOT NULL re-check.
        for (i, col) in table.columns.iter().enumerate() {
            if col.not_null && values[i].is_null() {
                return Err(SqlError::Constraint(format!(
                    "NOT NULL constraint failed: {}.{}",
                    table.name, col.name
                )));
            }
        }

        if new_rowid != rowid {
            tree.delete(tx, rowid)?;
        }
        let row = Row::new(new_rowid, with_alias_nulled(values, table.rowid_alias));
        tree.insert(tx, new_rowid, &row.encode())?;
        count += 1;
    }
    Ok(count)
}

pub fn delete(tx: &WriteTx, table: &Table, plan: &DeletePlan, params: &[Value]) -> Result<u64> {
    let tree = BTree::open(table.root_page);
    let matching = scan_matching(tx, &tree, plan.predicate.as_ref(), params, table.rowid_alias)?;
    let mut count = 0u64;
    for (rowid, _) in matching {
        if tree.delete(tx, rowid)? {
            count += 1;
        }
    }
    Ok(count)
}

/// Evaluate a column's `DEFAULT` for a fresh row.
fn eval_default(col: &Column) -> Value {
    match &col.default {
        None => Value::Null,
        Some(DefaultExpr::Value(v)) => v.clone(),
        Some(DefaultExpr::CurrentTimestamp) => Value::Text(now_utc("%Y-%m-%d %H:%M:%S")),
        Some(DefaultExpr::CurrentDate) => Value::Text(now_utc("%Y-%m-%d")),
        Some(DefaultExpr::CurrentTime) => Value::Text(now_utc("%H:%M:%S")),
    }
}

/// Find the first UNIQUE-column conflict for `values` (ignoring the rowid alias,
/// which the b-tree key already enforces). Returns `(column index, rowid)`.
fn find_unique_conflict(
    tx: &WriteTx,
    tree: &BTree,
    table: &Table,
    values: &[Value],
    exclude: Option<i64>,
) -> Result<Option<(usize, i64)>> {
    let unique_cols: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(i, c)| c.unique && Some(*i) != table.rowid_alias && !values[*i].is_null())
        .map(|(i, _)| i)
        .collect();
    if unique_cols.is_empty() {
        return Ok(None);
    }
    let mut cursor = tree.cursor(tx)?;
    while let Some((rowid, bytes)) = cursor.next()? {
        if Some(rowid) == exclude {
            continue;
        }
        let row = Row::decode(rowid, &bytes)?;
        for &ci in &unique_cols {
            if row.values.get(ci) == Some(&values[ci]) {
                return Ok(Some((ci, rowid)));
            }
        }
    }
    Ok(None)
}

/// Current UTC time formatted like SQLite's `CURRENT_*` defaults.
fn now_utc(fmt: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil date from Unix seconds (Howard Hinnant's algorithm).
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    fmt.replace("%Y", &format!("{year:04}"))
        .replace("%m", &format!("{m:02}"))
        .replace("%d", &format!("{d:02}"))
        .replace("%H", &format!("{hh:02}"))
        .replace("%M", &format!("{mm:02}"))
        .replace("%S", &format!("{ss:02}"))
}

/// Set the rowid-alias column (if any) to NULL before encoding, matching SQLite.
fn with_alias_nulled(mut values: Vec<Value>, alias: Option<usize>) -> Vec<Value> {
    if let Some(a) = alias {
        if a < values.len() {
            values[a] = Value::Null;
        }
    }
    values
}

/// Collect all rows whose predicate is truthy (all rows if there is none),
/// substituting the rowid into the rowid-alias column.
fn scan_matching(
    tx: &WriteTx,
    tree: &BTree,
    predicate: Option<&crate::planner::Expr>,
    params: &[Value],
    rowid_alias: Option<usize>,
) -> Result<Vec<(i64, Vec<Value>)>> {
    let mut cursor = tree.cursor(tx)?;
    let mut out = Vec::new();
    while let Some((rowid, bytes)) = cursor.next()? {
        let mut row = Row::decode(rowid, &bytes)?;
        if let Some(a) = rowid_alias {
            if a < row.values.len() {
                row.values[a] = Value::Integer(rowid);
            }
        }
        let keep = match predicate {
            None => true,
            Some(p) => eval(p, &row.values, rowid, params)?.is_truthy(),
        };
        if keep {
            out.push((rowid, row.values));
        }
    }
    Ok(out)
}
