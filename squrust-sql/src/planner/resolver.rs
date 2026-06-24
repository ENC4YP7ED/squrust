//! Name resolution: translate `sqlparser` expressions into resolved [`Expr`]s
//! against a column scope.

use sqlparser::ast::{
    BinaryOperator, Expr as SqlExpr, Function, FunctionArg, FunctionArgExpr, FunctionArguments,
    UnaryOperator,
};

use crate::ddl;
use crate::error::{Result, SqlError};
use crate::planner::AggExpr;
use crate::planner::expr::{BinOp, Expr, UnOp};
use crate::types::SqlType;

/// One column visible in the current scope.
#[derive(Debug, Clone)]
pub struct ScopeCol {
    pub table: Option<String>,
    pub name: String,
    pub sql_type: SqlType,
    pub decl_type: Option<String>,
}

/// The ordered set of columns available to expressions, matching the row layout.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub cols: Vec<ScopeCol>,
}

impl Scope {
    pub fn resolve(&self, table: Option<&str>, name: &str) -> Result<usize> {
        let mut found = None;
        for (i, c) in self.cols.iter().enumerate() {
            let name_ok = c.name.eq_ignore_ascii_case(name);
            let table_ok = match (table, &c.table) {
                (None, _) => true,
                (Some(t), Some(ct)) => t.eq_ignore_ascii_case(ct),
                (Some(_), None) => false,
            };
            if name_ok && table_ok {
                if found.is_some() {
                    return Err(SqlError::Ambiguous(format!("column `{name}`")));
                }
                found = Some(i);
            }
        }
        found.ok_or_else(|| SqlError::NotFound(format!("column `{name}`")))
    }
}

/// State threaded through resolution (parameter numbering).
#[derive(Default)]
pub struct ResolveCtx {
    pub next_param: usize,
}

pub fn is_aggregate_name(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "TOTAL"
    )
}

/// Does this expression tree contain an aggregate function call?
pub fn contains_aggregate(expr: &SqlExpr) -> bool {
    match expr {
        SqlExpr::Function(f) => {
            let name = ddl::object_name_to_string(&f.name).to_ascii_uppercase();
            let arg_count = match &f.args {
                FunctionArguments::List(list) => list.args.len(),
                _ => 0,
            };
            // min()/max() with ≥2 args are scalar, not aggregate.
            let is_agg = is_aggregate_name(&name)
                && !(matches!(name.as_str(), "MIN" | "MAX") && arg_count > 1);
            is_agg || {
                if let FunctionArguments::List(list) = &f.args {
                    list.args.iter().any(|a| match a {
                        FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                        | FunctionArg::Named {
                            arg: FunctionArgExpr::Expr(e),
                            ..
                        } => contains_aggregate(e),
                        _ => false,
                    })
                } else {
                    false
                }
            }
        }
        SqlExpr::BinaryOp { left, right, .. } => {
            contains_aggregate(left) || contains_aggregate(right)
        }
        SqlExpr::UnaryOp { expr, .. } | SqlExpr::Nested(expr) => contains_aggregate(expr),
        _ => false,
    }
}

/// Resolve a HAVING predicate over a per-group "augmented" row: input columns
/// are `Column(0..base_len)` (evaluated against a representative group row) and
/// each aggregate becomes `Column(base_len + i)` after being appended to `aggs`.
pub fn resolve_having(
    e: &SqlExpr,
    scope: &Scope,
    base_len: usize,
    aggs: &mut Vec<AggExpr>,
    ctx: &mut ResolveCtx,
    aliases: &std::collections::HashMap<String, Expr>,
) -> Result<Expr> {
    // An aggregate call anywhere in HAVING is computed per group and referenced
    // positionally in the augmented row.
    if let Some(agg) = super::try_aggregate(e, scope, ctx)? {
        aggs.push(agg);
        return Ok(Expr::Column(base_len + aggs.len() - 1));
    }
    match e {
        SqlExpr::Nested(inner) => resolve_having(inner, scope, base_len, aggs, ctx, aliases),
        SqlExpr::BinaryOp { left, op, right } => Ok(Expr::Binary {
            op: map_binop(op)?,
            left: Box::new(resolve_having(left, scope, base_len, aggs, ctx, aliases)?),
            right: Box::new(resolve_having(right, scope, base_len, aggs, ctx, aliases)?),
        }),
        SqlExpr::UnaryOp { op, expr } => {
            let inner = resolve_having(expr, scope, base_len, aggs, ctx, aliases)?;
            match op {
                UnaryOperator::Not => Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(inner),
                }),
                UnaryOperator::Minus => Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(inner),
                }),
                UnaryOperator::Plus => Ok(inner),
                other => Err(SqlError::Unsupported(format!("unary operator {other:?}"))),
            }
        }
        // A bare name may be a base column or a SELECT-list alias (SQLite allows
        // HAVING to reference output aliases). Base column wins.
        SqlExpr::Identifier(id) if scope.resolve(None, &id.value).is_err() => {
            match aliases.get(&id.value.to_ascii_lowercase()) {
                Some(ex) => Ok(ex.clone()),
                None => resolve_expr(e, scope, ctx),
            }
        }
        // A subtree with no aggregate resolves normally against the base scope.
        _ => resolve_expr(e, scope, ctx),
    }
}

pub fn resolve_expr(expr: &SqlExpr, scope: &Scope, ctx: &mut ResolveCtx) -> Result<Expr> {
    match expr {
        SqlExpr::Identifier(id) => {
            if id.value.eq_ignore_ascii_case("rowid") && scope.resolve(None, "rowid").is_err() {
                return Ok(Expr::RowId);
            }
            Ok(Expr::Column(scope.resolve(None, &id.value)?))
        }
        SqlExpr::CompoundIdentifier(parts) => {
            if parts.len() == 2 {
                Ok(Expr::Column(
                    scope.resolve(Some(&parts[0].value), &parts[1].value)?,
                ))
            } else {
                Err(SqlError::Unsupported(format!(
                    "compound identifier with {} parts",
                    parts.len()
                )))
            }
        }
        SqlExpr::Value(v) => match v {
            sqlparser::ast::Value::Placeholder(p) => Ok(Expr::Param(param_index(p, ctx))),
            other => Ok(Expr::Literal(ddl::value_from_sql(other)?)),
        },
        SqlExpr::Nested(inner) => resolve_expr(inner, scope, ctx),
        SqlExpr::BinaryOp { left, op, right } => {
            let l = resolve_expr(left, scope, ctx)?;
            let r = resolve_expr(right, scope, ctx)?;
            let op = map_binop(op)?;
            Ok(Expr::Binary {
                op,
                left: Box::new(l),
                right: Box::new(r),
            })
        }
        SqlExpr::UnaryOp { op, expr } => {
            let e = resolve_expr(expr, scope, ctx)?;
            let op = match op {
                UnaryOperator::Not => UnOp::Not,
                UnaryOperator::Minus => UnOp::Neg,
                UnaryOperator::Plus => return Ok(e),
                other => {
                    return Err(SqlError::Unsupported(format!("unary operator {other:?}")));
                }
            };
            Ok(Expr::Unary {
                op,
                expr: Box::new(e),
            })
        }
        SqlExpr::IsNull(e) => Ok(Expr::IsNull {
            expr: Box::new(resolve_expr(e, scope, ctx)?),
            negated: false,
        }),
        SqlExpr::IsNotNull(e) => Ok(Expr::IsNull {
            expr: Box::new(resolve_expr(e, scope, ctx)?),
            negated: true,
        }),
        SqlExpr::Like {
            negated,
            expr,
            pattern,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(resolve_expr(expr, scope, ctx)?),
            pattern: Box::new(resolve_expr(pattern, scope, ctx)?),
            negated: *negated,
        }),
        SqlExpr::InList {
            expr,
            list,
            negated,
        } => Ok(Expr::InList {
            expr: Box::new(resolve_expr(expr, scope, ctx)?),
            list: list
                .iter()
                .map(|e| resolve_expr(e, scope, ctx))
                .collect::<Result<_>>()?,
            negated: *negated,
        }),
        SqlExpr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            // a BETWEEN x AND y  ==>  a >= x AND a <= y
            let a = resolve_expr(expr, scope, ctx)?;
            let lo = resolve_expr(low, scope, ctx)?;
            let hi = resolve_expr(high, scope, ctx)?;
            let ge = Expr::Binary {
                op: BinOp::GtEq,
                left: Box::new(a.clone()),
                right: Box::new(lo),
            };
            let le = Expr::Binary {
                op: BinOp::LtEq,
                left: Box::new(a),
                right: Box::new(hi),
            };
            let and = Expr::Binary {
                op: BinOp::And,
                left: Box::new(ge),
                right: Box::new(le),
            };
            if *negated {
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(and),
                })
            } else {
                Ok(and)
            }
        }
        SqlExpr::Function(f) => resolve_scalar_function(f, scope, ctx),
        SqlExpr::Cast {
            expr, data_type, ..
        } => Ok(Expr::Cast {
            expr: Box::new(resolve_expr(expr, scope, ctx)?),
            ty: SqlType::affinity_from_decl(&data_type.to_string()),
        }),
        // TRIM is a SQL keyword parsed as a dedicated node.
        SqlExpr::Trim {
            expr,
            trim_where,
            trim_what,
            trim_characters,
        } => {
            use sqlparser::ast::TrimWhereField;
            let name = match trim_where {
                Some(TrimWhereField::Leading) => "LTRIM",
                Some(TrimWhereField::Trailing) => "RTRIM",
                _ => "TRIM",
            };
            let mut args = vec![resolve_expr(expr, scope, ctx)?];
            if let Some(w) = trim_what {
                args.push(resolve_expr(w, scope, ctx)?);
            } else if let Some(chars) = trim_characters {
                if let Some(first) = chars.first() {
                    args.push(resolve_expr(first, scope, ctx)?);
                }
            }
            Ok(Expr::Function {
                name: name.to_string(),
                args,
            })
        }
        SqlExpr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            let operand = match operand {
                Some(o) => Some(Box::new(resolve_expr(o, scope, ctx)?)),
                None => None,
            };
            let whens = conditions
                .iter()
                .zip(results)
                .map(|(c, r)| Ok((resolve_expr(c, scope, ctx)?, resolve_expr(r, scope, ctx)?)))
                .collect::<Result<Vec<_>>>()?;
            let else_result = match else_result {
                Some(e) => Some(Box::new(resolve_expr(e, scope, ctx)?)),
                None => None,
            };
            Ok(Expr::Case {
                operand,
                whens,
                else_result,
            })
        }
        SqlExpr::Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => {
            let mut args = vec![resolve_expr(expr, scope, ctx)?];
            if let Some(from) = substring_from {
                args.push(resolve_expr(from, scope, ctx)?);
            }
            if let Some(for_) = substring_for {
                args.push(resolve_expr(for_, scope, ctx)?);
            }
            Ok(Expr::Function {
                name: "SUBSTR".to_string(),
                args,
            })
        }
        other => Err(SqlError::Unsupported(format!("expression: {other}"))),
    }
}

fn resolve_scalar_function(f: &Function, scope: &Scope, ctx: &mut ResolveCtx) -> Result<Expr> {
    let name = ddl::object_name_to_string(&f.name).to_ascii_uppercase();
    let mut args = Vec::new();
    if let FunctionArguments::List(list) = &f.args {
        for a in &list.args {
            match a {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                | FunctionArg::Named {
                    arg: FunctionArgExpr::Expr(e),
                    ..
                } => args.push(resolve_expr(e, scope, ctx)?),
                _ => {
                    return Err(SqlError::Unsupported(
                        "wildcard argument to scalar function".into(),
                    ));
                }
            }
        }
    }
    Ok(Expr::Function { name, args })
}

fn param_index(placeholder: &str, ctx: &mut ResolveCtx) -> usize {
    // `?` is positional; `$N` / `:N` use the explicit 1-based number.
    let digits: String = placeholder.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Ok(n) = digits.parse::<usize>() {
        if n >= 1 {
            let idx = n - 1;
            ctx.next_param = ctx.next_param.max(n);
            return idx;
        }
    }
    let idx = ctx.next_param;
    ctx.next_param += 1;
    idx
}

fn map_binop(op: &BinaryOperator) -> Result<BinOp> {
    Ok(match op {
        BinaryOperator::Eq => BinOp::Eq,
        BinaryOperator::NotEq => BinOp::NotEq,
        BinaryOperator::Lt => BinOp::Lt,
        BinaryOperator::LtEq => BinOp::LtEq,
        BinaryOperator::Gt => BinOp::Gt,
        BinaryOperator::GtEq => BinOp::GtEq,
        BinaryOperator::And => BinOp::And,
        BinaryOperator::Or => BinOp::Or,
        BinaryOperator::Plus => BinOp::Add,
        BinaryOperator::Minus => BinOp::Sub,
        BinaryOperator::Multiply => BinOp::Mul,
        BinaryOperator::Divide => BinOp::Div,
        BinaryOperator::Modulo => BinOp::Mod,
        BinaryOperator::StringConcat => BinOp::Concat,
        other => return Err(SqlError::Unsupported(format!("operator {other:?}"))),
    })
}
