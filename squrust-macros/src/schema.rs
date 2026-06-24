//! Compile-time schema loading and SQL validation for `sql!` and `migrate!`.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sqlparser::ast::{Expr, Insert, ObjectName, Query, Select, SetExpr, Statement, TableFactor};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

fn obj_name(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|i| i.value.clone())
        .collect::<Vec<_>>()
        .join(".")
}

fn schema_path() -> Result<PathBuf, String> {
    if let Ok(p) = env::var("SQURUST_SCHEMA") {
        return Ok(PathBuf::from(p));
    }
    let manifest =
        env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR is not set".to_string())?;
    let p = Path::new(&manifest).join("squrust.schema");
    if p.exists() {
        Ok(p)
    } else {
        Err(format!(
            "schema file not found: set SQURUST_SCHEMA or create {}",
            p.display()
        ))
    }
}

fn load_schema() -> Result<HashMap<String, Vec<String>>, String> {
    let path = schema_path()?;
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read schema {}: {e}", path.display()))?;
    let stmts = Parser::parse_sql(&SQLiteDialect {}, &text)
        .map_err(|e| format!("schema parse error: {e}"))?;
    let mut map = HashMap::new();
    for s in stmts {
        if let Statement::CreateTable(ct) = s {
            let name = obj_name(&ct.name).to_lowercase();
            let cols = ct
                .columns
                .iter()
                .map(|c| c.name.value.to_lowercase())
                .collect();
            map.insert(name, cols);
        }
    }
    Ok(map)
}

pub fn validate_sql(query: &str) -> Result<(), String> {
    let schema = load_schema()?;
    let stmts = Parser::parse_sql(&SQLiteDialect {}, query)
        .map_err(|e| format!("SQL parse error: {e}"))?;

    for stmt in &stmts {
        let tables = referenced_tables(stmt);
        for t in &tables {
            if !schema.contains_key(&t.to_lowercase()) {
                return Err(format!("unknown table `{t}`"));
            }
        }
        // Column validation is only attempted for unambiguous single-table
        // statements, to avoid false positives on joins/aliases.
        if tables.len() == 1 {
            if let Some(cols) = schema.get(&tables[0].to_lowercase()) {
                let (idents, aliases) = column_identifiers(stmt);
                for ident in idents {
                    let l = ident.to_lowercase();
                    if l == "rowid" || aliases.contains(&l) || cols.contains(&l) {
                        continue;
                    }
                    return Err(format!(
                        "unknown column `{ident}` in table `{}`",
                        tables[0]
                    ));
                }
            }
        }
    }
    Ok(())
}

fn referenced_tables(stmt: &Statement) -> Vec<String> {
    let mut out = Vec::new();
    match stmt {
        Statement::Query(q) => collect_query_tables(q, &mut out),
        Statement::Insert(Insert { table_name, .. }) => out.push(obj_name(table_name)),
        Statement::Update { table, .. } => {
            if let TableFactor::Table { name, .. } = &table.relation {
                out.push(obj_name(name));
            }
        }
        Statement::Delete(del) => {
            use sqlparser::ast::FromTable;
            let from = match &del.from {
                FromTable::WithFromKeyword(t) | FromTable::WithoutKeyword(t) => t,
            };
            for twj in from {
                if let TableFactor::Table { name, .. } = &twj.relation {
                    out.push(obj_name(name));
                }
            }
        }
        _ => {}
    }
    out
}

fn collect_query_tables(q: &Query, out: &mut Vec<String>) {
    if let SetExpr::Select(select) = q.body.as_ref() {
        for twj in &select.from {
            if let TableFactor::Table { name, .. } = &twj.relation {
                out.push(obj_name(name));
            }
            for join in &twj.joins {
                if let TableFactor::Table { name, .. } = &join.relation {
                    out.push(obj_name(name));
                }
            }
        }
    }
}

/// Collect referenced column identifiers and projection aliases from a SELECT.
fn column_identifiers(stmt: &Statement) -> (Vec<String>, Vec<String>) {
    let mut idents = Vec::new();
    let mut aliases = Vec::new();
    if let Statement::Query(q) = stmt {
        if let SetExpr::Select(select) = q.body.as_ref() {
            collect_select_idents(select, &mut idents, &mut aliases);
        }
    }
    (idents, aliases.into_iter().map(|a| a.to_lowercase()).collect())
}

fn collect_select_idents(select: &Select, idents: &mut Vec<String>, aliases: &mut Vec<String>) {
    use sqlparser::ast::SelectItem;
    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(e) => collect_expr_idents(e, idents),
            SelectItem::ExprWithAlias { expr, alias } => {
                collect_expr_idents(expr, idents);
                aliases.push(alias.value.clone());
            }
            _ => {}
        }
    }
    if let Some(sel) = &select.selection {
        collect_expr_idents(sel, idents);
    }
}

fn collect_expr_idents(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Identifier(id) => out.push(id.value.clone()),
        Expr::CompoundIdentifier(parts) => {
            if let Some(last) = parts.last() {
                out.push(last.value.clone());
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_expr_idents(left, out);
            collect_expr_idents(right, out);
        }
        Expr::UnaryOp { expr, .. } | Expr::Nested(expr) | Expr::IsNull(expr)
        | Expr::IsNotNull(expr) => collect_expr_idents(expr, out),
        Expr::Like { expr, pattern, .. } => {
            collect_expr_idents(expr, out);
            collect_expr_idents(pattern, out);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_expr_idents(expr, out);
            collect_expr_idents(low, out);
            collect_expr_idents(high, out);
        }
        Expr::InList { expr, list, .. } => {
            collect_expr_idents(expr, out);
            for e in list {
                collect_expr_idents(e, out);
            }
        }
        // Functions: validate arguments but not the function name itself.
        Expr::Function(f) => {
            use sqlparser::ast::{FunctionArg, FunctionArgExpr, FunctionArguments};
            if let FunctionArguments::List(list) = &f.args {
                for a in &list.args {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(e),
                        ..
                    } = a
                    {
                        collect_expr_idents(e, out);
                    }
                }
            }
        }
        _ => {}
    }
}

pub struct MigrationEntry {
    pub version: u32,
    pub description: String,
    pub sql: String,
}

pub fn load_migrations(dir: &str) -> Result<Vec<MigrationEntry>, String> {
    let manifest =
        env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR is not set".to_string())?;
    let base = Path::new(&manifest).join(dir);
    let read = fs::read_dir(&base)
        .map_err(|e| format!("cannot read migrations dir {}: {e}", base.display()))?;

    let mut entries: Vec<MigrationEntry> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "invalid migration filename".to_string())?
            .to_string();
        let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
        let version: u32 = digits
            .parse()
            .map_err(|_| format!("migration `{stem}` has no numeric version prefix"))?;
        let description = stem
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['_', '-', ' '])
            .replace(['_', '-'], " ");
        let sql = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        entries.push(MigrationEntry {
            version,
            description,
            sql,
        });
    }

    entries.sort_by_key(|m| m.version);
    for pair in entries.windows(2) {
        if pair[0].version == pair[1].version {
            return Err(format!("duplicate migration version {}", pair[0].version));
        }
    }
    Ok(entries)
}
