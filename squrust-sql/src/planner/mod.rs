//! Logical planning: translate `sqlparser` statements into resolved plans.

pub mod expr;
pub mod optimizer;
pub mod resolver;

use sqlparser::ast::{
    Expr as SqlExpr, Insert, Query, Select, SelectItem, SetExpr, SqliteOnConflict, Statement,
    TableFactor,
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
}

#[derive(Debug, Clone)]
pub struct AggExpr {
    pub func: AggFunc,
    pub arg: Option<Expr>,
    pub distinct: bool,
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
}

impl LogicalPlan {
    pub fn columns(&self) -> Vec<ColumnInfo> {
        match self {
            LogicalPlan::Dual => vec![],
            LogicalPlan::Scan { columns, .. }
            | LogicalPlan::Project { columns, .. }
            | LogicalPlan::NestedLoopJoin { columns, .. }
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
    }
}

fn lookup<'a>(catalog: &'a Catalog, name: &str) -> Result<&'a Table> {
    catalog
        .get_table(name)
        .ok_or_else(|| SqlError::NotFound(format!("table `{name}`")))
}

fn plan_query(q: &Query, catalog: &Catalog, ctx: &mut ResolveCtx) -> Result<LogicalPlan> {
    let select = match q.body.as_ref() {
        SetExpr::Select(s) => s.as_ref(),
        other => return Err(SqlError::Unsupported(format!("query body: {other}"))),
    };

    // Build the FROM source and its scope.
    let (mut node, scope) = plan_from(select, catalog)?;

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

fn plan_from(select: &Select, catalog: &Catalog) -> Result<(LogicalPlan, Scope)> {
    if select.from.is_empty() {
        return Ok((LogicalPlan::Dual, Scope::default()));
    }
    if select.from.len() != 1 {
        return Err(SqlError::Unsupported("comma-joined tables".into()));
    }
    let twj = &select.from[0];
    let (left_table, left_alias) = table_factor(&twj.relation)?;
    let left = lookup(catalog, &left_table)?;
    let mut node = scan_for(left);
    let mut scope = aliased_scope(left, left_alias.as_deref());

    if twj.joins.is_empty() {
        return Ok((node, scope));
    }
    if twj.joins.len() != 1 {
        return Err(SqlError::Unsupported("more than one join".into()));
    }
    let join = &twj.joins[0];
    let (right_table, right_alias) = table_factor(&join.relation)?;
    let right = lookup(catalog, &right_table)?;
    let right_scan = scan_for(right);
    let right_scope = aliased_scope(right, right_alias.as_deref());

    // Combined scope: left columns then right columns.
    let mut combined = scope.clone();
    combined.cols.extend(right_scope.cols.clone());

    use sqlparser::ast::{JoinConstraint, JoinOperator};
    let mut ctx = ResolveCtx::default();
    let (predicate, left_outer) = match &join.join_operator {
        JoinOperator::Inner(JoinConstraint::On(e)) => {
            (Some(resolve_expr(e, &combined, &mut ctx)?), false)
        }
        JoinOperator::LeftOuter(JoinConstraint::On(e)) => {
            (Some(resolve_expr(e, &combined, &mut ctx)?), true)
        }
        JoinOperator::Inner(JoinConstraint::None) | JoinOperator::CrossJoin => (None, false),
        other => return Err(SqlError::Unsupported(format!("join type {other:?}"))),
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

    node = LogicalPlan::NestedLoopJoin {
        left: Box::new(node),
        right: Box::new(right_scan),
        predicate,
        left_outer,
        columns,
    };
    scope = combined;
    Ok((node, scope))
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
    let func = match name.as_str() {
        "COUNT" => AggFunc::Count,
        "SUM" | "TOTAL" => AggFunc::Sum,
        "AVG" => AggFunc::Avg,
        "MIN" => AggFunc::Min,
        "MAX" => AggFunc::Max,
        _ => unreachable!(),
    };
    Ok(Some(AggExpr {
        func,
        arg,
        distinct,
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
