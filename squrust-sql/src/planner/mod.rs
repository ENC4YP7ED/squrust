//! Logical planning: translate `sqlparser` statements into resolved plans.

pub mod expr;
pub mod optimizer;
pub mod resolver;

use sqlparser::ast::{
    AlterTableOperation, Expr as SqlExpr, Insert, ObjectName, Query, Select, SelectItem, SetExpr,
    SqliteOnConflict, Statement, TableFactor,
};

use squrust_core::PageId;

use crate::ddl;
use crate::error::{Result, SqlError};
use crate::schema::catalog::Catalog;
use crate::schema::{Index, Table};

pub use expr::{BinOp, Expr, UnOp};
use resolver::{Scope, ScopeCol, contains_aggregate, is_aggregate_name, resolve_expr, ResolveCtx};

/// Output column metadata for a plan node.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub table: Option<String>,
    /// Declared type (for `sqlite3_column_decltype`), if this output column maps
    /// directly to a table column.
    pub decl_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    CountStar,
    Sum,
    Avg,
    Min,
    Max,
    GroupConcat,
}

#[derive(Debug, Clone)]
pub struct AggExpr {
    pub func: AggFunc,
    pub arg: Option<Expr>,
    pub distinct: bool,
    /// `group_concat`'s optional separator expression (2nd argument). `None`
    /// means the default `,`. Ignored by every other aggregate.
    pub sep: Option<Expr>,
}

/// One output column of an aggregate node.
#[derive(Debug, Clone)]
pub enum OutputCol {
    /// A non-aggregate expression evaluated against a representative group row.
    Expr(Expr),
    /// The finalized value of `aggs[index]`.
    Agg(usize),
}

#[derive(Debug, Clone)]
pub struct SortKey {
    pub expr: Expr,
    pub asc: bool,
}

#[derive(Debug, Clone)]
pub enum LogicalPlan {
    /// A single synthetic row with no columns (for `SELECT <expr>` with no FROM).
    Dual,
    Scan {
        table: String,
        root_page: PageId,
        columns: Vec<ColumnInfo>,
        rowid_alias: Option<usize>,
        /// Per-column constant default, used to pad records that predate an
        /// `ALTER TABLE ADD COLUMN` (they have fewer stored columns).
        defaults: Vec<crate::types::Value>,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<LogicalPlan>,
        exprs: Vec<Expr>,
        columns: Vec<ColumnInfo>,
    },
    NestedLoopJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        predicate: Option<Expr>,
        left_outer: bool,
        columns: Vec<ColumnInfo>,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<Expr>,
        aggs: Vec<AggExpr>,
        output: Vec<OutputCol>,
        columns: Vec<ColumnInfo>,
        /// Number of input columns (offset where agg results begin in the
        /// per-group augmented row used to evaluate `having`).
        base_len: usize,
        /// HAVING predicate over the augmented row: input columns are
        /// `Column(0..base_len)`, aggregate `i` is `Column(base_len + i)`.
        having: Option<Expr>,
    },
    Sort {
        input: Box<LogicalPlan>,
        keys: Vec<SortKey>,
    },
    Limit {
        input: Box<LogicalPlan>,
        limit: Option<u64>,
        offset: u64,
    },
    Distinct {
        input: Box<LogicalPlan>,
    },
    /// A compound `UNION`/`INTERSECT`/`EXCEPT` of two sub-plans.
    SetOp {
        kind: SetOpKind,
        /// `UNION ALL` keeps duplicates; otherwise the result is distinct.
        all: bool,
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        columns: Vec<ColumnInfo>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOpKind {
    Union,
    Intersect,
    Except,
}

impl LogicalPlan {
    pub fn columns(&self) -> Vec<ColumnInfo> {
        match self {
            LogicalPlan::Dual => vec![],
            LogicalPlan::Scan { columns, .. }
            | LogicalPlan::Project { columns, .. }
            | LogicalPlan::NestedLoopJoin { columns, .. }
            | LogicalPlan::SetOp { columns, .. }
            | LogicalPlan::Aggregate { columns, .. } => columns.clone(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input } => input.columns(),
        }
    }
}

// ---- Statement-level plans ----

#[derive(Debug, Clone)]
pub struct InsertPlan {
    pub table: String,
    /// For each value position, the target table column index.
    pub target_cols: Vec<usize>,
    pub rows: Vec<Vec<Expr>>,
    pub or_replace: bool,
}

#[derive(Debug, Clone)]
pub struct UpdatePlan {
    pub table: String,
    pub assignments: Vec<(usize, Expr)>,
    pub predicate: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct DeletePlan {
    pub table: String,
    pub predicate: Option<Expr>,
}

#[derive(Debug, Clone)]
pub enum Plan {
    Query {
        plan: LogicalPlan,
        columns: Vec<ColumnInfo>,
    },
    Insert(InsertPlan),
    Update(UpdatePlan),
    Delete(DeletePlan),
    CreateTable {
        table: Table,
        if_not_exists: bool,
    },
    CreateIndex {
        index: Index,
        if_not_exists: bool,
    },
    DropTable {
        name: String,
        if_exists: bool,
    },
    AlterTableAddColumn {
        table: String,
        column: crate::schema::Column,
        /// Rewritten `CREATE TABLE` text for `sqlite_master`.
        new_sql: String,
    },
}

/// Translate one parsed statement into a [`Plan`].
pub fn plan(stmt: &Statement, catalog: &Catalog) -> Result<Plan> {
    let mut ctx = ResolveCtx::default();
    match stmt {
        Statement::Query(q) => {
            let plan = optimizer::optimize(plan_query(q, catalog, &mut ctx)?);
            let columns = plan.columns();
            Ok(Plan::Query { plan, columns })
        }
        Statement::Insert(insert) => plan_insert(insert, catalog, &mut ctx),
        Statement::Update {
            table,
            assignments,
            selection,
            ..
        } => plan_update(table, assignments, selection, catalog, &mut ctx),
        Statement::Delete(del) => plan_delete(del, catalog, &mut ctx),
        Statement::CreateTable(ct) => {
            let table = ddl::table_from_ast(ct, &stmt.to_string())?;
            Ok(Plan::CreateTable {
                table,
                if_not_exists: ct.if_not_exists,
            })
        }
        Statement::CreateIndex(ci) => {
            let index = ddl::index_from_ast(ci, &stmt.to_string())?;
            Ok(Plan::CreateIndex {
                index,
                if_not_exists: ci.if_not_exists,
            })
        }
        Statement::Drop {
            object_type,
            if_exists,
            names,
            ..
        } => {
            if !matches!(object_type, sqlparser::ast::ObjectType::Table) {
                return Err(SqlError::Unsupported(format!("DROP {object_type:?}")));
            }
            let name = ddl::object_name_to_string(
                names
                    .first()
                    .ok_or_else(|| SqlError::Parse("DROP with no name".into()))?,
            );
            Ok(Plan::DropTable {
                name,
                if_exists: *if_exists,
            })
        }
        Statement::AlterTable {
            name, operations, ..
        } => plan_alter_table(name, operations, catalog),
        other => Err(SqlError::Unsupported(format!("statement: {other}"))),
    }
}

fn table_scope(table: &Table) -> Scope {
    Scope {
        cols: table
            .columns
            .iter()
            .map(|c| ScopeCol {
                table: Some(table.name.clone()),
                name: c.name.clone(),
                sql_type: c.sql_type,
                decl_type: decl_opt(&c.decl_type),
            })
            .collect(),
    }
}

fn decl_opt(decl: &str) -> Option<String> {
    if decl.is_empty() {
        None
    } else {
        Some(decl.to_string())
    }
}

fn plan_alter_table(
    name: &ObjectName,
    operations: &[AlterTableOperation],
    catalog: &Catalog,
) -> Result<Plan> {
    let table_name = ddl::object_name_to_string(name);
    let table = lookup(catalog, &table_name)?;

    // SQLite applies one operation per ALTER TABLE; we support ADD COLUMN.
    let op = match operations {
        [op] => op,
        _ => {
            return Err(SqlError::Unsupported(
                "only a single ALTER TABLE operation is supported".into(),
            ));
        }
    };
    let (column_def, if_not_exists) = match op {
        AlterTableOperation::AddColumn {
            column_def,
            if_not_exists,
            ..
        } => (column_def, *if_not_exists),
        other => {
            return Err(SqlError::Unsupported(format!("ALTER TABLE {other}")));
        }
    };

    let column = ddl::column_from_ast(column_def);

    if table.column(&column.name).is_some() {
        if if_not_exists {
            // No-op: report success without rewriting anything.
            return Ok(Plan::AlterTableAddColumn {
                table: table_name,
                column,
                new_sql: table.sql.clone(),
            });
        }
        return Err(SqlError::Constraint(format!(
            "duplicate column name: {}",
            column.name
        )));
    }

    // SQLite's restrictions on ADD COLUMN.
    if column.primary_key {
        return Err(SqlError::Unsupported(
            "cannot add a PRIMARY KEY column".into(),
        ));
    }
    if column.unique {
        return Err(SqlError::Unsupported("cannot add a UNIQUE column".into()));
    }
    match &column.default {
        // A NOT NULL column needs a default to backfill existing rows.
        None if column.not_null => {
            return Err(SqlError::Constraint(
                "cannot add a NOT NULL column with no default value".into(),
            ));
        }
        // The default must be a constant (CURRENT_* / expressions are rejected,
        // matching SQLite — old rows are padded with this constant on read).
        Some(crate::schema::DefaultExpr::Value(_)) | None => {}
        Some(_) => {
            return Err(SqlError::Unsupported(
                "cannot add a column with a non-constant default".into(),
            ));
        }
    }

    // Splice the new column definition into the stored CREATE TABLE text, just
    // before its closing paren — exactly how SQLite rewrites sqlite_master.
    let old_sql = &table.sql;
    let pos = old_sql
        .rfind(')')
        .ok_or_else(|| SqlError::Schema("malformed CREATE TABLE text".into()))?;
    let mut new_sql = String::with_capacity(old_sql.len() + 32);
    new_sql.push_str(old_sql[..pos].trim_end());
    new_sql.push_str(", ");
    new_sql.push_str(&column_def.to_string());
    new_sql.push_str(&old_sql[pos..]);

    Ok(Plan::AlterTableAddColumn {
        table: table_name,
        column,
        new_sql,
    })
}

fn scan_for(table: &Table) -> LogicalPlan {
    LogicalPlan::Scan {
        table: table.name.clone(),
        root_page: table.root_page,
        columns: table
            .columns
            .iter()
            .map(|c| ColumnInfo {
                name: c.name.clone(),
                table: Some(table.name.clone()),
                decl_type: decl_opt(&c.decl_type),
            })
            .collect(),
        rowid_alias: table.rowid_alias,
        defaults: table
            .columns
            .iter()
            .map(|c| match &c.default {
                Some(crate::schema::DefaultExpr::Value(v)) => v.clone(),
                _ => crate::types::Value::Null,
            })
            .collect(),
    }
}

fn lookup<'a>(catalog: &'a Catalog, name: &str) -> Result<&'a Table> {
    catalog
        .get_table(name)
        .ok_or_else(|| SqlError::NotFound(format!("table `{name}`")))
}

/// A named common-table-expression: a pre-planned subquery usable in `FROM`.
#[derive(Clone)]
struct CteDef {
    plan: LogicalPlan,
    scope: Scope,
}

type CteMap = std::collections::HashMap<String, CteDef>;

fn plan_query(q: &Query, catalog: &Catalog, ctx: &mut ResolveCtx) -> Result<LogicalPlan> {
    plan_query_with(q, catalog, ctx, &CteMap::new())
}

/// Plan a query, with any `WITH` (CTE) clause and inherited CTEs in scope.
fn plan_query_with(
    q: &Query,
    catalog: &Catalog,
    ctx: &mut ResolveCtx,
    inherited: &CteMap,
) -> Result<LogicalPlan> {
    let ctes = build_ctes(q, catalog, ctx, inherited)?;

    let select = match q.body.as_ref() {
        SetExpr::Select(s) => s.as_ref(),
        SetExpr::SetOperation { .. } => {
            // Compound query: build the set-op tree, then ORDER BY / LIMIT.
            let node = plan_set_expr(&q.body, catalog, ctx, &ctes)?;
            return apply_order_limit(node, q, ctx, OrderScope::Output);
        }
        other => return Err(SqlError::Unsupported(format!("query body: {other}"))),
    };

    // Build the FROM source and its scope.
    let (mut node, scope) = plan_from(select, catalog, &ctes)?;

    // WHERE
    if let Some(pred) = &select.selection {
        let predicate = resolve_expr(pred, &scope, ctx)?;
        node = LogicalPlan::Filter {
            input: Box::new(node),
            predicate,
        };
    }

    // Detect aggregation.
    let group_exprs: &[SqlExpr] = match &select.group_by {
        sqlparser::ast::GroupByExpr::Expressions(exprs, _) => exprs,
        _ => &[],
    };
    let has_agg = !group_exprs.is_empty()
        || select.projection.iter().any(|item| match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                contains_aggregate(e)
            }
            _ => false,
        });

    let distinct = match &select.distinct {
        None => false,
        Some(sqlparser::ast::Distinct::Distinct) => true,
        Some(other) => return Err(SqlError::Unsupported(format!("{other}"))),
    };

    if has_agg {
        node = plan_aggregate(node, select, group_exprs, &scope, ctx)?;
        if distinct {
            node = LogicalPlan::Distinct {
                input: Box::new(node),
            };
        }
        node = apply_order_limit(node, q, ctx, OrderScope::Output)?;
    } else {
        // ORDER BY runs on base rows (before projection).
        let projected_exprs = collect_projection(select, &scope, ctx)?;
        if let Some(order) = &q.order_by {
            let keys = plan_sort_keys(&order.exprs, &scope, &projected_exprs, ctx)?;
            node = LogicalPlan::Sort {
                input: Box::new(node),
                keys,
            };
        }
        let (exprs, columns): (Vec<Expr>, Vec<ColumnInfo>) = projected_exprs.into_iter().unzip();
        node = LogicalPlan::Project {
            input: Box::new(node),
            exprs,
            columns,
        };
        if distinct {
            node = LogicalPlan::Distinct {
                input: Box::new(node),
            };
        }
        node = apply_limit(node, q, ctx)?;
    }

    Ok(node)
}

/// Plan a `SELECT` body without `ORDER BY`/`LIMIT` — used as a set-operation
/// operand (compound operands carry no `ORDER BY`/`LIMIT` of their own).
fn plan_select_core(
    select: &Select,
    catalog: &Catalog,
    ctx: &mut ResolveCtx,
    ctes: &CteMap,
) -> Result<LogicalPlan> {
    let (mut node, scope) = plan_from(select, catalog, ctes)?;

    if let Some(pred) = &select.selection {
        let predicate = resolve_expr(pred, &scope, ctx)?;
        node = LogicalPlan::Filter {
            input: Box::new(node),
            predicate,
        };
    }

    let group_exprs: &[SqlExpr] = match &select.group_by {
        sqlparser::ast::GroupByExpr::Expressions(exprs, _) => exprs,
        _ => &[],
    };
    let has_agg = !group_exprs.is_empty()
        || select.projection.iter().any(|item| match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                contains_aggregate(e)
            }
            _ => false,
        });

    if has_agg {
        node = plan_aggregate(node, select, group_exprs, &scope, ctx)?;
    } else {
        let projected = collect_projection(select, &scope, ctx)?;
        let (exprs, columns): (Vec<Expr>, Vec<ColumnInfo>) = projected.into_iter().unzip();
        node = LogicalPlan::Project {
            input: Box::new(node),
            exprs,
            columns,
        };
    }

    let distinct = match &select.distinct {
        None => false,
        Some(sqlparser::ast::Distinct::Distinct) => true,
        Some(other) => return Err(SqlError::Unsupported(format!("{other}"))),
    };
    if distinct {
        node = LogicalPlan::Distinct {
            input: Box::new(node),
        };
    }
    Ok(node)
}

/// Plan a set-expression operand: a `SELECT`, a nested set operation, or a
/// parenthesized query.
fn plan_set_expr(
    se: &SetExpr,
    catalog: &Catalog,
    ctx: &mut ResolveCtx,
    ctes: &CteMap,
) -> Result<LogicalPlan> {
    use sqlparser::ast::{SetOperator, SetQuantifier};
    match se {
        SetExpr::Select(s) => plan_select_core(s, catalog, ctx, ctes),
        SetExpr::Query(q) => plan_query_with(q, catalog, ctx, ctes),
        SetExpr::SetOperation {
            op,
            set_quantifier,
            left,
            right,
        } => {
            let l = plan_set_expr(left, catalog, ctx, ctes)?;
            let r = plan_set_expr(right, catalog, ctx, ctes)?;
            if l.columns().len() != r.columns().len() {
                return Err(SqlError::Schema(
                    "SELECTs to the left and right of a set operator do not have the same number of result columns".into(),
                ));
            }
            let kind = match op {
                SetOperator::Union => SetOpKind::Union,
                SetOperator::Intersect => SetOpKind::Intersect,
                SetOperator::Except => SetOpKind::Except,
            };
            let all = matches!(set_quantifier, SetQuantifier::All | SetQuantifier::AllByName);
            let columns = l.columns();
            Ok(LogicalPlan::SetOp {
                kind,
                all,
                left: Box::new(l),
                right: Box::new(r),
                columns,
            })
        }
        other => Err(SqlError::Unsupported(format!("set expression: {other}"))),
    }
}

/// Build the CTE map for a query from its `WITH` clause (plus inherited CTEs).
/// Each CTE body is planned with earlier CTEs visible. Recursive CTEs are
/// rejected.
fn build_ctes(
    q: &Query,
    catalog: &Catalog,
    ctx: &mut ResolveCtx,
    inherited: &CteMap,
) -> Result<CteMap> {
    let mut map = inherited.clone();
    let with = match &q.with {
        Some(w) => w,
        None => return Ok(map),
    };
    if with.recursive {
        return Err(SqlError::Unsupported(
            "recursive CTEs (WITH RECURSIVE) are not supported".into(),
        ));
    }
    for cte in &with.cte_tables {
        let plan = plan_query_with(&cte.query, catalog, ctx, &map)?;
        let mut cols = plan.columns();
        if !cte.alias.columns.is_empty() {
            if cte.alias.columns.len() != cols.len() {
                return Err(SqlError::Schema(format!(
                    "CTE `{}` declares {} columns but the query returns {}",
                    cte.alias.name.value,
                    cte.alias.columns.len(),
                    cols.len()
                )));
            }
            for (c, ident) in cols.iter_mut().zip(&cte.alias.columns) {
                c.name = ident.value.clone();
            }
        }
        let name = cte.alias.name.value.clone();
        let scope = Scope {
            cols: cols
                .iter()
                .map(|c| ScopeCol {
                    table: Some(name.clone()),
                    name: c.name.clone(),
                    sql_type: crate::types::SqlType::Null,
                    decl_type: c.decl_type.clone(),
                })
                .collect(),
        };
        map.insert(name.to_ascii_lowercase(), CteDef { plan, scope });
    }
    Ok(map)
}

/// Plan a subquery `(SELECT ...)` into a logical plan. Used by the engine to
/// pre-evaluate non-correlated subqueries. Bind parameters inside a subquery
/// aren't supported (their numbering would collide with the outer query).
pub fn plan_subquery(q: &Query, catalog: &Catalog) -> Result<LogicalPlan> {
    let mut ctx = ResolveCtx::default();
    let plan = optimizer::optimize(plan_query(q, catalog, &mut ctx)?);
    if ctx.next_param > 0 {
        return Err(SqlError::Unsupported(
            "bind parameters inside subqueries are not supported".into(),
        ));
    }
    Ok(plan)
}

fn plan_from(
    select: &Select,
    catalog: &Catalog,
    ctes: &CteMap,
) -> Result<(LogicalPlan, Scope)> {
    if select.from.is_empty() {
        return Ok((LogicalPlan::Dual, Scope::default()));
    }
    let mut ctx = ResolveCtx::default();
    // Build a left-deep nested-loop tree. Comma-separated `FROM` entries are
    // cross-joined; explicit `JOIN`s within an entry fold in left-to-right.
    let mut acc: Option<(LogicalPlan, Scope)> = None;
    for twj in &select.from {
        let (scan, sc) = resolve_relation(&twj.relation, catalog, ctes)?;
        acc = Some(match acc {
            None => (scan, sc),
            // A new FROM entry is an implicit cross join (no ON predicate).
            Some((ln, ls)) => join_step(ln, ls, scan, sc, None, &mut ctx)?,
        });
        for join in &twj.joins {
            let (jscan, jsc) = resolve_relation(&join.relation, catalog, ctes)?;
            let (ln, ls) = acc.take().expect("seeded above");
            acc = Some(join_step(ln, ls, jscan, jsc, Some(&join.join_operator), &mut ctx)?);
        }
    }
    Ok(acc.expect("non-empty FROM"))
}

/// Resolve a `FROM`/`JOIN` relation to a plan node and its scope. A name that
/// matches a CTE inlines the CTE's plan; otherwise it's a catalog table scan.
fn resolve_relation(
    tf: &TableFactor,
    catalog: &Catalog,
    ctes: &CteMap,
) -> Result<(LogicalPlan, Scope)> {
    let (name, alias) = table_factor(tf)?;
    if let Some(cte) = ctes.get(&name.to_ascii_lowercase()) {
        // Inline the CTE, qualifying its columns by the alias (or CTE name).
        let qualifier = alias.unwrap_or(name);
        let mut scope = cte.scope.clone();
        for c in &mut scope.cols {
            c.table = Some(qualifier.clone());
        }
        return Ok((cte.plan.clone(), scope));
    }
    let tbl = lookup(catalog, &name)?;
    Ok((scan_for(tbl), aliased_scope(tbl, alias.as_deref())))
}

/// Fold one relation into the accumulated join tree. `op` is `None` for an
/// implicit (comma) cross join, otherwise the explicit join operator.
fn join_step(
    left_node: LogicalPlan,
    left_scope: Scope,
    right_scan: LogicalPlan,
    right_scope: Scope,
    op: Option<&sqlparser::ast::JoinOperator>,
    ctx: &mut ResolveCtx,
) -> Result<(LogicalPlan, Scope)> {
    use sqlparser::ast::{JoinConstraint, JoinOperator};

    // Combined scope: left columns then right columns (used to resolve ON).
    let mut combined = left_scope;
    combined.cols.extend(right_scope.cols);

    let (predicate, left_outer) = match op {
        None | Some(JoinOperator::Inner(JoinConstraint::None)) | Some(JoinOperator::CrossJoin) => {
            (None, false)
        }
        Some(JoinOperator::Inner(JoinConstraint::On(e))) => {
            (Some(resolve_expr(e, &combined, ctx)?), false)
        }
        Some(JoinOperator::LeftOuter(JoinConstraint::On(e))) => {
            (Some(resolve_expr(e, &combined, ctx)?), true)
        }
        Some(other) => return Err(SqlError::Unsupported(format!("join type {other:?}"))),
    };

    let columns: Vec<ColumnInfo> = combined
        .cols
        .iter()
        .map(|c| ColumnInfo {
            name: c.name.clone(),
            table: c.table.clone(),
            decl_type: c.decl_type.clone(),
        })
        .collect();

    let node = LogicalPlan::NestedLoopJoin {
        left: Box::new(left_node),
        right: Box::new(right_scan),
        predicate,
        left_outer,
        columns,
    };
    Ok((node, combined))
}

fn aliased_scope(table: &Table, alias: Option<&str>) -> Scope {
    let mut scope = table_scope(table);
    if let Some(a) = alias {
        for c in &mut scope.cols {
            c.table = Some(a.to_string());
        }
    }
    scope
}

fn table_factor(tf: &TableFactor) -> Result<(String, Option<String>)> {
    match tf {
        TableFactor::Table { name, alias, .. } => Ok((
            ddl::object_name_to_string(name),
            alias.as_ref().map(|a| a.name.value.clone()),
        )),
        other => Err(SqlError::Unsupported(format!("FROM factor: {other}"))),
    }
}

/// Resolve the SELECT list into (expr, column-info) pairs, expanding wildcards.
fn collect_projection(
    select: &Select,
    scope: &Scope,
    ctx: &mut ResolveCtx,
) -> Result<Vec<(Expr, ColumnInfo)>> {
    let mut out = Vec::new();
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => {
                for (i, c) in scope.cols.iter().enumerate() {
                    out.push((
                        Expr::Column(i),
                        ColumnInfo {
                            name: c.name.clone(),
                            table: c.table.clone(),
                            decl_type: c.decl_type.clone(),
                        },
                    ));
                }
            }
            SelectItem::QualifiedWildcard(obj, _) => {
                let t = ddl::object_name_to_string(obj);
                for (i, c) in scope.cols.iter().enumerate() {
                    if c.table.as_deref().map(|x| x.eq_ignore_ascii_case(&t)) == Some(true) {
                        out.push((
                            Expr::Column(i),
                            ColumnInfo {
                                name: c.name.clone(),
                                table: c.table.clone(),
                                decl_type: c.decl_type.clone(),
                            },
                        ));
                    }
                }
            }
            SelectItem::UnnamedExpr(e) => {
                let expr = resolve_expr(e, scope, ctx)?;
                let decl_type = decl_for_expr(&expr, scope);
                out.push((
                    expr,
                    ColumnInfo {
                        name: derive_name(e),
                        table: None,
                        decl_type,
                    },
                ));
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                let expr = resolve_expr(expr, scope, ctx)?;
                let decl_type = decl_for_expr(&expr, scope);
                out.push((
                    expr,
                    ColumnInfo {
                        name: alias.value.clone(),
                        table: None,
                        decl_type,
                    },
                ));
            }
        }
    }
    Ok(out)
}

/// Declared type for a projected expression, if it's a direct column reference.
fn decl_for_expr(expr: &Expr, scope: &Scope) -> Option<String> {
    match expr {
        Expr::Column(i) => scope.cols.get(*i).and_then(|c| c.decl_type.clone()),
        _ => None,
    }
}

fn derive_name(e: &SqlExpr) -> String {
    match e {
        SqlExpr::Identifier(id) => id.value.clone(),
        SqlExpr::CompoundIdentifier(parts) => {
            parts.last().map(|p| p.value.clone()).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

fn plan_aggregate(
    input: LogicalPlan,
    select: &Select,
    group_exprs: &[SqlExpr],
    scope: &Scope,
    ctx: &mut ResolveCtx,
) -> Result<LogicalPlan> {
    let group_by: Vec<Expr> = group_exprs
        .iter()
        .map(|e| resolve_expr(e, scope, ctx))
        .collect::<Result<_>>()?;

    let base_len = scope.cols.len();
    let mut aggs: Vec<AggExpr> = Vec::new();
    let mut output: Vec<OutputCol> = Vec::new();
    let mut columns: Vec<ColumnInfo> = Vec::new();
    // alias (lowercased) -> its value in augmented-row terms, for HAVING.
    let mut aliases: std::collections::HashMap<String, Expr> = std::collections::HashMap::new();

    for item in &select.projection {
        let (sql_expr, name) = match item {
            SelectItem::UnnamedExpr(e) => (e, derive_name(e)),
            SelectItem::ExprWithAlias { expr, alias } => (expr, alias.value.clone()),
            other => {
                return Err(SqlError::Unsupported(format!(
                    "wildcard with aggregation: {other}"
                )));
            }
        };
        if let Some(agg) = try_aggregate(sql_expr, scope, ctx)? {
            aggs.push(agg);
            let idx = aggs.len() - 1;
            output.push(OutputCol::Agg(idx));
            aliases.insert(name.to_ascii_lowercase(), Expr::Column(base_len + idx));
        } else {
            let resolved = resolve_expr(sql_expr, scope, ctx)?;
            aliases.insert(name.to_ascii_lowercase(), resolved.clone());
            output.push(OutputCol::Expr(resolved));
        }
        columns.push(ColumnInfo {
            name,
            table: None,
            decl_type: None,
        });
    }

    let having = match &select.having {
        Some(h) => Some(resolver::resolve_having(
            h, scope, base_len, &mut aggs, ctx, &aliases,
        )?),
        None => None,
    };

    Ok(LogicalPlan::Aggregate {
        input: Box::new(input),
        group_by,
        aggs,
        output,
        columns,
        base_len,
        having,
    })
}

fn try_aggregate(e: &SqlExpr, scope: &Scope, ctx: &mut ResolveCtx) -> Result<Option<AggExpr>> {
    use sqlparser::ast::{FunctionArg, FunctionArgExpr, FunctionArguments};
    let SqlExpr::Function(f) = e else {
        return Ok(None);
    };
    let name = ddl::object_name_to_string(&f.name).to_ascii_uppercase();
    if !is_aggregate_name(&name) {
        return Ok(None);
    }
    let (distinct, args) = match &f.args {
        FunctionArguments::List(list) => (
            matches!(
                list.duplicate_treatment,
                Some(sqlparser::ast::DuplicateTreatment::Distinct)
            ),
            &list.args[..],
        ),
        _ => (false, &[][..]),
    };

    // min()/max() with two or more arguments are *scalar* functions, not
    // aggregates (SQLite semantics). Defer them to the scalar evaluator.
    if matches!(name.as_str(), "MIN" | "MAX") && args.len() > 1 {
        return Ok(None);
    }

    // COUNT(*) has a single wildcard argument.
    if name == "COUNT"
        && args.len() == 1
        && matches!(
            &args[0],
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard)
                | FunctionArg::Unnamed(FunctionArgExpr::QualifiedWildcard(_))
        )
    {
        return Ok(Some(AggExpr {
            func: AggFunc::CountStar,
            arg: None,
            distinct,
            sep: None,
        }));
    }

    let arg = match args.first() {
        Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(e))) => Some(resolve_expr(e, scope, ctx)?),
        None => None,
        _ => {
            return Err(SqlError::Unsupported(format!(
                "argument to aggregate {name}"
            )));
        }
    };

    // group_concat(X [, SEP]) — the optional 2nd argument is the separator.
    let sep = if name == "GROUP_CONCAT" {
        match args.get(1) {
            Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(e))) => {
                Some(resolve_expr(e, scope, ctx)?)
            }
            None => None,
            _ => return Err(SqlError::Unsupported("separator to group_concat".into())),
        }
    } else {
        None
    };

    let func = match name.as_str() {
        "COUNT" => AggFunc::Count,
        "SUM" | "TOTAL" => AggFunc::Sum,
        "AVG" => AggFunc::Avg,
        "MIN" => AggFunc::Min,
        "MAX" => AggFunc::Max,
        "GROUP_CONCAT" => AggFunc::GroupConcat,
        _ => unreachable!(),
    };
    Ok(Some(AggExpr {
        func,
        arg,
        distinct,
        sep,
    }))
}

enum OrderScope {
    Output,
}

fn apply_order_limit(
    mut node: LogicalPlan,
    q: &Query,
    ctx: &mut ResolveCtx,
    _scope: OrderScope,
) -> Result<LogicalPlan> {
    if let Some(order) = &q.order_by {
        let out_cols = node.columns();
        let out_scope = Scope {
            cols: out_cols
                .iter()
                .map(|c| ScopeCol {
                    table: c.table.clone(),
                    name: c.name.clone(),
                    sql_type: crate::types::SqlType::Null,
                    decl_type: c.decl_type.clone(),
                })
                .collect(),
        };
        let mut keys = Vec::new();
        for ob in &order.exprs {
            let expr = order_key_expr(&ob.expr, &out_scope, None, ctx)?;
            keys.push(SortKey {
                expr,
                asc: ob.asc.unwrap_or(true),
            });
        }
        node = LogicalPlan::Sort {
            input: Box::new(node),
            keys,
        };
    }
    apply_limit(node, q, ctx)
}

fn plan_sort_keys(
    order_exprs: &[sqlparser::ast::OrderByExpr],
    base_scope: &Scope,
    projected: &[(Expr, ColumnInfo)],
    ctx: &mut ResolveCtx,
) -> Result<Vec<SortKey>> {
    let mut keys = Vec::new();
    for ob in order_exprs {
        let expr = order_key_expr(&ob.expr, base_scope, Some(projected), ctx)?;
        keys.push(SortKey {
            expr,
            asc: ob.asc.unwrap_or(true),
        });
    }
    Ok(keys)
}

/// Resolve an ORDER BY expression, supporting ordinals (`ORDER BY 2`).
fn order_key_expr(
    e: &SqlExpr,
    scope: &Scope,
    projected: Option<&[(Expr, ColumnInfo)]>,
    ctx: &mut ResolveCtx,
) -> Result<Expr> {
    if let SqlExpr::Value(sqlparser::ast::Value::Number(n, _)) = e {
        if let Ok(idx) = n.parse::<usize>() {
            if idx >= 1 {
                if let Some(proj) = projected {
                    // Non-aggregate: sort runs on base rows, so reuse the
                    // projection expression at that ordinal.
                    return proj
                        .get(idx - 1)
                        .map(|(ex, _)| ex.clone())
                        .ok_or_else(|| SqlError::NotFound(format!("ORDER BY position {idx}")));
                }
                // Aggregate output: reference the output column directly.
                return Ok(Expr::Column(idx - 1));
            }
        }
    }
    // Allow ORDER BY to reference a projection alias by name (non-aggregate).
    if let (SqlExpr::Identifier(id), Some(proj)) = (e, projected) {
        if scope.resolve(None, &id.value).is_err() {
            if let Some((ex, _)) = proj
                .iter()
                .find(|(_, ci)| ci.name.eq_ignore_ascii_case(&id.value))
            {
                return Ok(ex.clone());
            }
        }
    }
    resolve_expr(e, scope, ctx)
}

fn apply_limit(mut node: LogicalPlan, q: &Query, ctx: &mut ResolveCtx) -> Result<LogicalPlan> {
    let limit = match &q.limit {
        Some(e) => Some(const_u64(e, ctx)?),
        None => None,
    };
    let offset = match &q.offset {
        Some(o) => const_u64(&o.value, ctx)?,
        None => 0,
    };
    if limit.is_some() || offset > 0 {
        node = LogicalPlan::Limit {
            input: Box::new(node),
            limit,
            offset,
        };
    }
    Ok(node)
}

fn const_u64(e: &SqlExpr, _ctx: &mut ResolveCtx) -> Result<u64> {
    match e {
        SqlExpr::Value(sqlparser::ast::Value::Number(n, _)) => n
            .parse::<u64>()
            .map_err(|_| SqlError::Type(format!("bad LIMIT/OFFSET value {n}"))),
        other => Err(SqlError::Unsupported(format!(
            "non-constant LIMIT/OFFSET: {other}"
        ))),
    }
}

fn plan_insert(insert: &Insert, catalog: &Catalog, ctx: &mut ResolveCtx) -> Result<Plan> {
    let name = ddl::object_name_to_string(&insert.table_name);
    let table = lookup(catalog, &name)?;

    let target_cols: Vec<usize> = if insert.columns.is_empty() {
        (0..table.columns.len()).collect()
    } else {
        insert
            .columns
            .iter()
            .map(|c| {
                table
                    .column_index(&c.value)
                    .ok_or_else(|| SqlError::NotFound(format!("column `{}`", c.value)))
            })
            .collect::<Result<_>>()?
    };

    let source = insert
        .source
        .as_ref()
        .ok_or_else(|| SqlError::Unsupported("INSERT without VALUES".into()))?;
    let rows = match source.body.as_ref() {
        SetExpr::Values(values) => {
            let empty = Scope::default();
            values
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|e| resolve_expr(e, &empty, ctx))
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?
        }
        other => return Err(SqlError::Unsupported(format!("INSERT source: {other}"))),
    };

    let or_replace =
        matches!(insert.or, Some(SqliteOnConflict::Replace)) || insert.replace_into;

    Ok(Plan::Insert(InsertPlan {
        table: table.name.clone(),
        target_cols,
        rows,
        or_replace,
    }))
}

fn plan_update(
    table: &sqlparser::ast::TableWithJoins,
    assignments: &[sqlparser::ast::Assignment],
    selection: &Option<SqlExpr>,
    catalog: &Catalog,
    ctx: &mut ResolveCtx,
) -> Result<Plan> {
    let (name, _) = table_factor(&table.relation)?;
    let table = lookup(catalog, &name)?;
    let scope = table_scope(table);

    let mut resolved = Vec::new();
    for a in assignments {
        let col_name = match &a.target {
            sqlparser::ast::AssignmentTarget::ColumnName(obj) => ddl::object_name_to_string(obj),
            other => return Err(SqlError::Unsupported(format!("assignment target {other}"))),
        };
        let idx = table
            .column_index(&col_name)
            .ok_or_else(|| SqlError::NotFound(format!("column `{col_name}`")))?;
        resolved.push((idx, resolve_expr(&a.value, &scope, ctx)?));
    }

    let predicate = match selection {
        Some(e) => Some(resolve_expr(e, &scope, ctx)?),
        None => None,
    };

    Ok(Plan::Update(UpdatePlan {
        table: table.name.clone(),
        assignments: resolved,
        predicate,
    }))
}

fn plan_delete(del: &sqlparser::ast::Delete, catalog: &Catalog, ctx: &mut ResolveCtx) -> Result<Plan> {
    use sqlparser::ast::FromTable;
    let from = match &del.from {
        FromTable::WithFromKeyword(t) | FromTable::WithoutKeyword(t) => t,
    };
    let twj = from
        .first()
        .ok_or_else(|| SqlError::Parse("DELETE without table".into()))?;
    let (name, _) = table_factor(&twj.relation)?;
    let table = lookup(catalog, &name)?;
    let scope = table_scope(table);
    let predicate = match &del.selection {
        Some(e) => Some(resolve_expr(e, &scope, ctx)?),
        None => None,
    };
    Ok(Plan::Delete(DeletePlan {
        table: table.name.clone(),
        predicate,
    }))
}
