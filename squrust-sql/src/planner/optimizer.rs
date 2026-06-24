//! A small set of logical rewrites. Currently: constant folding of expression
//! trees. Predicate push-down and index selection are intentionally minimal —
//! the executor always uses table scans.

use crate::planner::expr::{BinOp, Expr, UnOp};
use crate::planner::{LogicalPlan, OutputCol, SortKey};
use crate::types::Value;

pub fn optimize(plan: LogicalPlan) -> LogicalPlan {
    fold_plan(plan)
}

fn fold_plan(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Filter { input, predicate } => LogicalPlan::Filter {
            input: Box::new(fold_plan(*input)),
            predicate: fold(predicate),
        },
        LogicalPlan::Project {
            input,
            exprs,
            columns,
        } => LogicalPlan::Project {
            input: Box::new(fold_plan(*input)),
            exprs: exprs.into_iter().map(fold).collect(),
            columns,
        },
        LogicalPlan::NestedLoopJoin {
            left,
            right,
            predicate,
            left_outer,
            columns,
        } => LogicalPlan::NestedLoopJoin {
            left: Box::new(fold_plan(*left)),
            right: Box::new(fold_plan(*right)),
            predicate: predicate.map(fold),
            left_outer,
            columns,
        },
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggs,
            output,
            columns,
            base_len,
            having,
        } => LogicalPlan::Aggregate {
            input: Box::new(fold_plan(*input)),
            group_by: group_by.into_iter().map(fold).collect(),
            aggs,
            output: output
                .into_iter()
                .map(|o| match o {
                    OutputCol::Expr(e) => OutputCol::Expr(fold(e)),
                    other => other,
                })
                .collect(),
            columns,
            base_len,
            having: having.map(fold),
        },
        LogicalPlan::Sort { input, keys } => LogicalPlan::Sort {
            input: Box::new(fold_plan(*input)),
            keys: keys
                .into_iter()
                .map(|k| SortKey {
                    expr: fold(k.expr),
                    asc: k.asc,
                })
                .collect(),
        },
        LogicalPlan::Limit {
            input,
            limit,
            offset,
        } => LogicalPlan::Limit {
            input: Box::new(fold_plan(*input)),
            limit,
            offset,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(fold_plan(*input)),
        },
        leaf => leaf,
    }
}

/// Constant-fold an expression where both operands are literals.
pub fn fold(expr: Expr) -> Expr {
    match expr {
        Expr::Binary { op, left, right } => {
            let l = fold(*left);
            let r = fold(*right);
            if let (Expr::Literal(a), Expr::Literal(b)) = (&l, &r) {
                if let Some(v) = eval_const_binary(op, a, b) {
                    return Expr::Literal(v);
                }
            }
            Expr::Binary {
                op,
                left: Box::new(l),
                right: Box::new(r),
            }
        }
        Expr::Unary { op, expr } => {
            let e = fold(*expr);
            if let Expr::Literal(v) = &e {
                if let Some(folded) = eval_const_unary(op, v) {
                    return Expr::Literal(folded);
                }
            }
            Expr::Unary {
                op,
                expr: Box::new(e),
            }
        }
        other => other,
    }
}

fn eval_const_binary(op: BinOp, a: &Value, b: &Value) -> Option<Value> {
    // Only fold the operators whose result type doesn't depend on SQLite's
    // integer-vs-real division rules; Div/Mod are left to the runtime evaluator.
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            let r = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                _ => unreachable!(),
            };
            if matches!(a, Value::Integer(_)) && matches!(b, Value::Integer(_)) && r.fract() == 0.0
            {
                Some(Value::Integer(r as i64))
            } else {
                Some(Value::Real(r))
            }
        }
        _ => None,
    }
}

fn eval_const_unary(op: UnOp, v: &Value) -> Option<Value> {
    match op {
        UnOp::Neg => match v {
            Value::Integer(i) => Some(Value::Integer(-i)),
            Value::Real(r) => Some(Value::Real(-r)),
            _ => None,
        },
        UnOp::Not => Some(Value::Boolean(!v.is_truthy())),
    }
}
