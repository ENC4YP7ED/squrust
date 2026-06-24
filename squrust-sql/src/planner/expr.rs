//! Resolved expression tree (column references resolved to row indices).

use crate::types::{SqlType, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Value),
    /// Column at this index in the current row's value list.
    Column(usize),
    /// The row id of the current row (e.g. `rowid` pseudo-column).
    RowId,
    /// Positional bind parameter (0-based).
    Param(usize),
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Function {
        name: String,
        args: Vec<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: SqlType,
    },
    Case {
        /// `CASE <operand> WHEN ...` (simple form); `None` for searched form.
        operand: Option<Box<Expr>>,
        /// `(when_condition, then_result)` pairs.
        whens: Vec<(Expr, Expr)>,
        else_result: Option<Box<Expr>>,
    },
}
