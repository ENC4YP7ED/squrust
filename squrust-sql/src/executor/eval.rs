//! Expression evaluation against a row of values.

use std::cmp::Ordering;

use crate::error::{Result, SqlError};
use crate::planner::expr::{BinOp, Expr, UnOp};
use crate::types::Value;

/// Evaluate `expr` against a row's `values` (and its `row_id`), with positional
/// `params` for bind placeholders.
pub fn eval(expr: &Expr, values: &[Value], row_id: i64, params: &[Value]) -> Result<Value> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Column(i) => values
            .get(*i)
            .cloned()
            .ok_or_else(|| SqlError::Type(format!("column index {i} out of range"))),
        Expr::RowId => Ok(Value::Integer(row_id)),
        Expr::Param(i) => params
            .get(*i)
            .cloned()
            .ok_or_else(|| SqlError::Type(format!("missing bind parameter ${}", i + 1))),
        Expr::Unary { op, expr } => {
            let v = eval(expr, values, row_id, params)?;
            Ok(match op {
                UnOp::Neg => match v {
                    Value::Integer(i) => Value::Integer(-i),
                    Value::Real(r) => Value::Real(-r),
                    Value::Null => Value::Null,
                    other => Value::Real(-other.as_f64().unwrap_or(0.0)),
                },
                UnOp::Not => {
                    if v.is_null() {
                        Value::Null
                    } else {
                        Value::Boolean(!v.is_truthy())
                    }
                }
            })
        }
        Expr::Binary { op, left, right } => {
            let l = eval(left, values, row_id, params)?;
            let r = eval(right, values, row_id, params)?;
            eval_binary(*op, l, r)
        }
        Expr::IsNull { expr, negated } => {
            let v = eval(expr, values, row_id, params)?;
            Ok(Value::Boolean(v.is_null() != *negated))
        }
        Expr::Like {
            expr,
            pattern,
            negated,
        } => {
            let v = eval(expr, values, row_id, params)?;
            let p = eval(pattern, values, row_id, params)?;
            if v.is_null() || p.is_null() {
                return Ok(Value::Null);
            }
            let m = like_match(&p.to_display_string(), &v.to_display_string());
            Ok(Value::Boolean(m != *negated))
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let v = eval(expr, values, row_id, params)?;
            if v.is_null() {
                return Ok(Value::Null);
            }
            let mut found = false;
            for item in list {
                let iv = eval(item, values, row_id, params)?;
                if v.compare(&iv) == Some(Ordering::Equal) {
                    found = true;
                    break;
                }
            }
            Ok(Value::Boolean(found != *negated))
        }
        Expr::Function { name, args } => {
            let argv = args
                .iter()
                .map(|a| eval(a, values, row_id, params))
                .collect::<Result<Vec<_>>>()?;
            eval_function(name, &argv)
        }
        Expr::Cast { expr, ty } => {
            let v = eval(expr, values, row_id, params)?;
            Ok(cast(v, *ty))
        }
        Expr::Case {
            operand,
            whens,
            else_result,
        } => {
            let op_val = match operand {
                Some(o) => Some(eval(o, values, row_id, params)?),
                None => None,
            };
            for (cond, result) in whens {
                let matched = match &op_val {
                    // Simple form: CASE x WHEN y -> x = y
                    Some(ov) => {
                        let cv = eval(cond, values, row_id, params)?;
                        ov.compare(&cv) == Some(Ordering::Equal)
                    }
                    // Searched form: CASE WHEN cond -> cond is truthy
                    None => eval(cond, values, row_id, params)?.is_truthy(),
                };
                if matched {
                    return eval(result, values, row_id, params);
                }
            }
            match else_result {
                Some(e) => eval(e, values, row_id, params),
                None => Ok(Value::Null),
            }
        }
        // Non-correlated subqueries are evaluated to constants before execution;
        // anything reaching here is correlated, which isn't supported yet.
        Expr::ScalarSubquery(_) | Expr::Exists { .. } | Expr::InSubquery { .. } => Err(
            SqlError::Unsupported("correlated subqueries are not supported".into()),
        ),
    }
}

/// SQLite `CAST(expr AS type)` — a forced conversion (e.g. `CAST(3.5 AS INTEGER)`
/// truncates to 3), distinct from column affinity.
fn cast(v: Value, ty: crate::types::SqlType) -> Value {
    use crate::types::SqlType;
    if v.is_null() {
        return Value::Null;
    }
    match ty {
        SqlType::Integer => match &v {
            Value::Integer(i) => Value::Integer(*i),
            Value::Boolean(b) => Value::Integer(*b as i64),
            Value::Text(t) => Value::Integer(leading_number(t).map(|f| f as i64).unwrap_or(0)),
            _ => Value::Integer(v.as_f64().map(|f| f as i64).unwrap_or(0)),
        },
        SqlType::Real => match &v {
            Value::Text(t) => Value::Real(leading_number(t).unwrap_or(0.0)),
            _ => Value::Real(v.as_f64().unwrap_or(0.0)),
        },
        SqlType::Text => match v {
            Value::Text(_) => v,
            other => Value::Text(other.to_display_string()),
        },
        SqlType::Boolean => Value::Boolean(v.is_truthy()),
        // BLOB/NONE/JSON cast keeps the value as-is.
        _ => v,
    }
}

/// Parse the longest leading numeric prefix of `s` (SQLite text→number rules).
fn leading_number(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if saw_digit && i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let mut exp_digit = false;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
            exp_digit = true;
        }
        if exp_digit {
            i = j;
        }
    }
    if !saw_digit {
        return None;
    }
    s[..i].parse::<f64>().ok()
}

fn eval_binary(op: BinOp, l: Value, r: Value) -> Result<Value> {
    match op {
        BinOp::And => {
            // Three-valued logic.
            match (truth(&l), truth(&r)) {
                (Some(false), _) | (_, Some(false)) => Ok(Value::Boolean(false)),
                (Some(true), Some(true)) => Ok(Value::Boolean(true)),
                _ => Ok(Value::Null),
            }
        }
        BinOp::Or => match (truth(&l), truth(&r)) {
            (Some(true), _) | (_, Some(true)) => Ok(Value::Boolean(true)),
            (Some(false), Some(false)) => Ok(Value::Boolean(false)),
            _ => Ok(Value::Null),
        },
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            if l.is_null() || r.is_null() {
                return Ok(Value::Null);
            }
            let ord = l
                .compare(&r)
                .ok_or_else(|| SqlError::Type("incomparable values".into()))?;
            let res = match op {
                BinOp::Eq => ord == Ordering::Equal,
                BinOp::NotEq => ord != Ordering::Equal,
                BinOp::Lt => ord == Ordering::Less,
                BinOp::LtEq => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                BinOp::GtEq => ord != Ordering::Less,
                _ => unreachable!(),
            };
            Ok(Value::Boolean(res))
        }
        BinOp::Concat => {
            if l.is_null() || r.is_null() {
                return Ok(Value::Null);
            }
            Ok(Value::Text(format!(
                "{}{}",
                l.to_display_string(),
                r.to_display_string()
            )))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            if l.is_null() || r.is_null() {
                return Ok(Value::Null);
            }
            let both_int = matches!(l, Value::Integer(_)) && matches!(r, Value::Integer(_));
            let (x, y) = (
                l.as_f64()
                    .ok_or_else(|| SqlError::Type("non-numeric operand".into()))?,
                r.as_f64()
                    .ok_or_else(|| SqlError::Type("non-numeric operand".into()))?,
            );
            let res = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => {
                    if y == 0.0 {
                        return Ok(Value::Null);
                    }
                    if both_int {
                        return Ok(Value::Integer((x as i64) / (y as i64)));
                    }
                    x / y
                }
                BinOp::Mod => {
                    if y == 0.0 {
                        return Ok(Value::Null);
                    }
                    if both_int {
                        return Ok(Value::Integer((x as i64) % (y as i64)));
                    }
                    x % y
                }
                _ => unreachable!(),
            };
            if both_int && res.fract() == 0.0 {
                Ok(Value::Integer(res as i64))
            } else {
                Ok(Value::Real(res))
            }
        }
    }
}

fn truth(v: &Value) -> Option<bool> {
    if v.is_null() {
        None
    } else {
        Some(v.is_truthy())
    }
}

fn eval_function(name: &str, args: &[Value]) -> Result<Value> {
    let upper = name.to_ascii_uppercase();
    Ok(match upper.as_str() {
        "LENGTH" => match args.first() {
            Some(Value::Null) | None => Value::Null,
            Some(v) => Value::Integer(v.to_display_string().chars().count() as i64),
        },
        "UPPER" => match args.first() {
            Some(Value::Null) | None => Value::Null,
            Some(v) => Value::Text(v.to_display_string().to_uppercase()),
        },
        "LOWER" => match args.first() {
            Some(Value::Null) | None => Value::Null,
            Some(v) => Value::Text(v.to_display_string().to_lowercase()),
        },
        "ABS" => match args.first().and_then(|v| v.as_f64()) {
            Some(f) => {
                if args.iter().all(|v| matches!(v, Value::Integer(_))) {
                    Value::Integer(f.abs() as i64)
                } else {
                    Value::Real(f.abs())
                }
            }
            None => Value::Null,
        },
        // Scalar (multi-arg) min/max: NULL if any argument is NULL, else the
        // extreme by SQLite value ordering. (Single-arg min/max are aggregates.)
        "MIN" | "MAX" if !args.is_empty() => {
            if args.iter().any(|v| v.is_null()) {
                Value::Null
            } else {
                let want_min = upper == "MIN";
                let mut best = args[0].clone();
                for v in &args[1..] {
                    let take = match v.compare(&best) {
                        Some(Ordering::Less) => want_min,
                        Some(Ordering::Greater) => !want_min,
                        _ => false,
                    };
                    if take {
                        best = v.clone();
                    }
                }
                best
            }
        }
        // Date/time functions (SQLite-compatible; see `datetime` module).
        "DATE" => super::datetime::date(args),
        "TIME" => super::datetime::time(args),
        "DATETIME" => super::datetime::datetime(args),
        "JULIANDAY" => super::datetime::julianday(args),
        "UNIXEPOCH" => super::datetime::unixepoch(args),
        "STRFTIME" => super::datetime::strftime(args),
        "COALESCE" => args
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        "IFNULL" => {
            let a = args.first().cloned().unwrap_or(Value::Null);
            if a.is_null() {
                args.get(1).cloned().unwrap_or(Value::Null)
            } else {
                a
            }
        }
        "ROUND" => match args.first().and_then(|v| v.as_f64()) {
            Some(f) => {
                let digits = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
                let factor = 10f64.powi(digits as i32);
                Value::Real((f * factor).round() / factor)
            }
            None => Value::Null,
        },
        "TYPEOF" => Value::Text(
            match args.first() {
                Some(Value::Null) | None => "null",
                Some(Value::Integer(_)) | Some(Value::Boolean(_)) => "integer",
                Some(Value::Real(_)) => "real",
                Some(Value::Text(_)) | Some(Value::Json(_)) => "text",
                Some(Value::Blob(_)) => "blob",
            }
            .to_string(),
        ),
        "SUBSTR" | "SUBSTRING" => substr(args),
        "REPLACE" => {
            if args.len() < 3 || args[..3].iter().any(|v| v.is_null()) {
                Value::Null
            } else {
                let (s, f, t) = (
                    args[0].to_display_string(),
                    args[1].to_display_string(),
                    args[2].to_display_string(),
                );
                if f.is_empty() {
                    Value::Text(s)
                } else {
                    Value::Text(s.replace(&f, &t))
                }
            }
        }
        "TRIM" => trim_fn(args, true, true),
        "LTRIM" => trim_fn(args, true, false),
        "RTRIM" => trim_fn(args, false, true),
        "INSTR" => {
            if args.len() < 2 || args[0].is_null() || args[1].is_null() {
                Value::Null
            } else {
                let hay = args[0].to_display_string();
                let needle = args[1].to_display_string();
                match hay.find(&needle) {
                    Some(b) => Value::Integer(hay[..b].chars().count() as i64 + 1),
                    None => Value::Integer(0),
                }
            }
        }
        "HEX" => match args.first() {
            Some(Value::Null) | None => Value::Null,
            Some(Value::Blob(b)) => Value::Text(b.iter().map(|x| format!("{x:02X}")).collect()),
            Some(v) => Value::Text(
                v.to_display_string()
                    .as_bytes()
                    .iter()
                    .map(|x| format!("{x:02X}"))
                    .collect(),
            ),
        },
        "CHAR" => Value::Text(
            args.iter()
                .filter_map(|v| v.as_i64())
                .filter_map(|i| char::from_u32(i as u32))
                .collect(),
        ),
        "UNICODE" => match args.first() {
            Some(Value::Null) | None => Value::Null,
            Some(v) => match v.to_display_string().chars().next() {
                Some(c) => Value::Integer(c as i64),
                None => Value::Null,
            },
        },
        "NULLIF" => {
            let a = args.first().cloned().unwrap_or(Value::Null);
            let b = args.get(1).cloned().unwrap_or(Value::Null);
            if !a.is_null() && a == b {
                Value::Null
            } else {
                a
            }
        }
        "SIGN" => match args.first().and_then(|v| v.as_f64()) {
            Some(f) => Value::Integer((f > 0.0) as i64 - (f < 0.0) as i64),
            None => Value::Null,
        },
        "QUOTE" => Value::Text(match args.first() {
            Some(Value::Null) | None => "NULL".to_string(),
            Some(Value::Integer(i)) => i.to_string(),
            Some(Value::Boolean(b)) => (*b as i64).to_string(),
            Some(Value::Real(r)) => r.to_string(),
            Some(other) => format!("'{}'", other.to_display_string().replace('\'', "''")),
        }),
        other => return Err(SqlError::Unsupported(format!("function {other}()"))),
    })
}

/// SQLite `substr(X, Y [, Z])` with 1-based, end-relative indexing.
fn substr(args: &[Value]) -> Value {
    let s = match args.first() {
        Some(Value::Null) | None => return Value::Null,
        Some(v) => v.to_display_string(),
    };
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let mut start = args.get(1).and_then(|v| v.as_i64()).unwrap_or(1);
    if start < 0 {
        start += n + 1;
    }
    if start < 1 {
        start = 1;
    }
    let start_idx = (start - 1).clamp(0, n) as usize;
    let result: String = match args.get(2) {
        Some(v) if v.is_null() => return Value::Null,
        Some(v) => {
            let len = v.as_i64().unwrap_or(0);
            if len < 0 {
                let take = (-len) as usize;
                let begin = start_idx.saturating_sub(take);
                chars[begin..start_idx].iter().collect()
            } else {
                chars.iter().skip(start_idx).take(len as usize).collect()
            }
        }
        None => chars.iter().skip(start_idx).collect(),
    };
    Value::Text(result)
}

fn trim_fn(args: &[Value], left: bool, right: bool) -> Value {
    match args.first() {
        Some(Value::Null) | None => return Value::Null,
        _ => {}
    }
    let s = args[0].to_display_string();
    let cutset: Vec<char> = match args.get(1) {
        Some(v) if !v.is_null() => v.to_display_string().chars().collect(),
        _ => vec![' '],
    };
    let mut out = s.as_str();
    if left {
        out = out.trim_start_matches(|c| cutset.contains(&c));
    }
    if right {
        out = out.trim_end_matches(|c| cutset.contains(&c));
    }
    Value::Text(out.to_string())
}

/// SQL `LIKE` matching with `%` (any run) and `_` (single char), case-insensitive
/// like SQLite's default ASCII behaviour.
fn like_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    like_rec(&p, &t)
}

fn like_rec(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '%' => {
            // Match zero or more characters.
            like_rec(&p[1..], t) || (!t.is_empty() && like_rec(p, &t[1..]))
        }
        '_' => !t.is_empty() && like_rec(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && like_rec(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns() {
        assert!(like_match("hel%", "hello"));
        assert!(like_match("h_llo", "hello"));
        assert!(like_match("%lo", "hello"));
        assert!(!like_match("hx%", "hello"));
        assert!(like_match("%", "anything"));
    }

    #[test]
    fn arithmetic_and_logic() {
        let v = eval_binary(BinOp::Add, Value::Integer(2), Value::Integer(3)).unwrap();
        assert_eq!(v, Value::Integer(5));
        let v = eval_binary(BinOp::And, Value::Boolean(true), Value::Null).unwrap();
        assert!(v.is_null());
        let v = eval_binary(BinOp::Or, Value::Boolean(true), Value::Null).unwrap();
        assert_eq!(v, Value::Boolean(true));
    }
}
