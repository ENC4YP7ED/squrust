//! Parsing of `CREATE TABLE` / `CREATE INDEX` into the schema model. Shared by
//! DDL execution and by catalog loading on open.

use sqlparser::ast::{
    ColumnOption, CreateIndex, CreateTable, Expr as SqlExpr, Statement, Value as SqlValue,
};

use crate::error::{Result, SqlError};
use crate::parser;
use crate::schema::{Column, Index, Table};
use crate::types::{SqlType, Value};

/// Parse a `CREATE TABLE` statement (by text) into a [`Table`] template. The
/// `root_page` is filled in by the caller.
pub fn parse_create_table(sql: &str) -> Result<Table> {
    let stmts = parser::parse(sql)?;
    let stmt = stmts
        .into_iter()
        .next()
        .ok_or_else(|| SqlError::Parse("empty statement".into()))?;
    match stmt {
        Statement::CreateTable(ct) => table_from_ast(&ct, sql),
        _ => Err(SqlError::Schema("expected CREATE TABLE".into())),
    }
}

pub fn table_from_ast(ct: &CreateTable, sql: &str) -> Result<Table> {
    let name = object_name_to_string(&ct.name);
    let mut columns = Vec::with_capacity(ct.columns.len());
    let mut rowid_alias = None;

    for (idx, col) in ct.columns.iter().enumerate() {
        let sql_type = SqlType::affinity_from_decl(&col.data_type.to_string());
        let mut not_null = false;
        let mut primary_key = false;
        let mut default = None;

        for opt in &col.options {
            match &opt.option {
                ColumnOption::NotNull => not_null = true,
                ColumnOption::Unique { is_primary, .. } if *is_primary => {
                    primary_key = true;
                    not_null = true;
                }
                ColumnOption::Default(expr) => {
                    default = Some(literal_from_expr(expr)?);
                }
                _ => {}
            }
        }

        if primary_key && sql_type == SqlType::Integer && rowid_alias.is_none() {
            rowid_alias = Some(idx);
        }

        columns.push(Column {
            name: col.name.value.clone(),
            sql_type,
            not_null,
            primary_key,
            default,
        });
    }

    Ok(Table {
        name,
        columns,
        root_page: 0,
        rowid_alias,
        sql: sql.to_string(),
    })
}

pub fn parse_create_index(sql: &str) -> Result<Index> {
    let stmts = parser::parse(sql)?;
    match stmts.into_iter().next() {
        Some(Statement::CreateIndex(ci)) => index_from_ast(&ci, sql),
        _ => Err(SqlError::Schema("expected CREATE INDEX".into())),
    }
}

pub fn index_from_ast(ci: &CreateIndex, sql: &str) -> Result<Index> {
    let name = ci
        .name
        .as_ref()
        .map(object_name_to_string)
        .ok_or_else(|| SqlError::Schema("index requires a name".into()))?;
    let table = object_name_to_string(&ci.table_name);
    let columns = ci
        .columns
        .iter()
        .map(|c| match &c.expr {
            SqlExpr::Identifier(id) => Ok(id.value.clone()),
            other => Err(SqlError::Unsupported(format!(
                "index on expression `{other}` is not supported"
            ))),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Index {
        name,
        table,
        columns,
        unique: ci.unique,
        sql: sql.to_string(),
    })
}

pub fn object_name_to_string(name: &sqlparser::ast::ObjectName) -> String {
    name.0
        .iter()
        .map(|i| i.value.clone())
        .collect::<Vec<_>>()
        .join(".")
}

/// Best-effort evaluation of a literal expression (for column defaults).
pub fn literal_from_expr(expr: &SqlExpr) -> Result<Value> {
    match expr {
        SqlExpr::Value(v) => value_from_sql(v),
        SqlExpr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr,
        } => match literal_from_expr(expr)? {
            Value::Integer(i) => Ok(Value::Integer(-i)),
            Value::Real(r) => Ok(Value::Real(-r)),
            other => Ok(other),
        },
        other => Err(SqlError::Unsupported(format!(
            "non-literal default expression: {other}"
        ))),
    }
}

pub fn value_from_sql(v: &SqlValue) -> Result<Value> {
    Ok(match v {
        SqlValue::Number(s, _) => {
            if let Ok(i) = s.parse::<i64>() {
                Value::Integer(i)
            } else {
                Value::Real(
                    s.parse::<f64>()
                        .map_err(|_| SqlError::Type(format!("bad number literal {s}")))?,
                )
            }
        }
        SqlValue::SingleQuotedString(s)
        | SqlValue::DoubleQuotedString(s)
        | SqlValue::EscapedStringLiteral(s) => Value::Text(s.clone()),
        SqlValue::HexStringLiteral(s) => Value::Blob(decode_hex(s)?),
        SqlValue::Boolean(b) => Value::Boolean(*b),
        SqlValue::Null => Value::Null,
        other => {
            return Err(SqlError::Unsupported(format!(
                "unsupported literal: {other}"
            )));
        }
    })
}

/// Decode a `x'...'` blob literal's hex digits into bytes.
fn decode_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(SqlError::Type("odd-length hex literal".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| SqlError::Type(format!("invalid hex literal x'{s}'")))
        })
        .collect()
}
